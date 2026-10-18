//! Pieces shared by the offscreen capture and the interactive viewer: DLSS setup, the
//! camera under test, the label overlay and the render-world probe.

use std::sync::{Arc, Mutex};

use bevy::{
    camera::{Hdr, MainPassResolutionOverride, RenderTarget},
    core_pipeline::{
        prepass::{DeferredPrepass, DepthPrepass, MotionVectorPrepass},
        tonemapping::Tonemapping,
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        render_resource::{CachedPipelineState, PipelineCache},
        renderer::RenderAdapterInfo,
        view::ExtractedView,
    },
};

use crate::{
    meta::Meta,
    scenes::{self, SceneCtx, SceneKind},
    variant::Variant,
};

#[cfg(feature = "dlss")]
const DLSS_PROJECT_ID: bevy::asset::uuid::Uuid =
    bevy::asset::uuid::Uuid::from_u128(0x6f1c2e4a_9b3d_4e7f_8a21_5c0d9e3b7a14);

/// Must run before `DefaultPlugins` are added.
pub fn insert_dlss_project_id(app: &mut App) {
    #[cfg(feature = "dlss")]
    app.insert_resource(bevy::anti_alias::dlss::DlssProjectId(DLSS_PROJECT_ID));
    #[cfg(not(feature = "dlss"))]
    let _ = app;
}

/// `Ok` if DLSS Super Resolution can be used, else why not.
#[derive(Resource, Clone, Copy)]
pub struct DlssAvailability(pub Result<(), &'static str>);

/// Adds [`DlssAvailability`] and the render-world probe.
pub struct BenchCorePlugin;

impl Plugin for BenchCorePlugin {
    fn build(&self, app: &mut App) {
        let probe = RenderProbe::default();
        app.insert_resource(probe.clone())
            .add_systems(PreStartup, detect_dlss);
        app.sub_app_mut(RenderApp)
            .insert_resource(probe)
            .add_systems(Render, probe_render_world.in_set(RenderSystems::Cleanup));
    }
}

fn detect_dlss(
    mut commands: Commands,
    #[cfg(feature = "dlss")] supported: Option<
        Res<bevy::anti_alias::dlss::DlssSuperResolutionSupported>,
    >,
) {
    #[cfg(feature = "dlss")]
    let availability = match supported {
        Some(_) => Ok(()),
        None => Err("DLSS Super Resolution not supported by this GPU / driver / backend"),
    };
    #[cfg(not(feature = "dlss"))]
    let availability = Err("binary built without the `dlss` cargo feature");
    commands.insert_resource(DlssAvailability(availability));
}

/// Written by the render world, read by the main world.
#[derive(Default, Debug, Clone, Copy)]
pub struct RenderProbeData {
    /// Size of the 3D camera's main pass: `MainPassResolutionOverride` if present, else the viewport.
    pub main_pass: Option<UVec2>,
    pub has_override: bool,
    pub pipelines_pending: usize,
}

#[derive(Resource, Clone, Default)]
pub struct RenderProbe(pub Arc<Mutex<RenderProbeData>>);

impl RenderProbe {
    pub fn get(&self) -> RenderProbeData {
        *self.0.lock().unwrap()
    }
}

fn probe_render_world(
    probe: Res<RenderProbe>,
    views: Query<(&ExtractedView, Option<&MainPassResolutionOverride>), With<Camera3d>>,
    pipeline_cache: Res<PipelineCache>,
) {
    let mut data = probe.0.lock().unwrap();
    data.pipelines_pending = pipeline_cache.waiting_pipelines().count()
        + pipeline_cache
            .pipelines()
            .filter(|p| {
                matches!(
                    p.state,
                    CachedPipelineState::Queued | CachedPipelineState::Creating(_)
                )
            })
            .count();
    match views.iter().next() {
        Some((view, resolution_override)) => {
            data.has_override = resolution_override.is_some();
            data.main_pass = Some(resolution_override.map(|o| o.0).unwrap_or(view.viewport.zw()));
        }
        None => {
            data.has_override = false;
            data.main_pass = None;
        }
    }
}

pub fn adapter_string(adapter: &RenderAdapterInfo) -> String {
    if adapter.driver_info.is_empty() {
        format!("{} ({:?})", adapter.name, adapter.backend)
    } else {
        format!("{} ({:?}, {})", adapter.name, adapter.backend, adapter.driver_info)
    }
}

pub fn base_meta(scene: SceneKind, variant: &Variant, output: UVec2, adapter: String) -> Meta {
    let info = scene.info();
    Meta {
        status: "ok".into(),
        scene: info.name.into(),
        title: info.title.into(),
        feature: info.feature.into(),
        variant: variant.name(),
        variant_desc: variant.describe(),
        expected: scene.expected_for(variant).into(),
        pipeline: if scene.uses_deferred(variant) { "deferred" } else { "forward" }.into(),
        output,
        render: None,
        adapter,
        features: Vec::new(),
        frames: 0,
    }
}

/// The renderer method must be set before the scene's materials are created.
pub fn opaque_method(scene: SceneKind, variant: &Variant) -> bevy::pbr::DefaultOpaqueRendererMethod {
    if scene.uses_deferred(variant) {
        bevy::pbr::DefaultOpaqueRendererMethod::deferred()
    } else {
        bevy::pbr::DefaultOpaqueRendererMethod::forward()
    }
}

/// Marks the 3D camera under test.
#[derive(Component)]
pub struct BenchCamera;

/// Spawns the camera under test and the scene. Returns the camera and the list of features on.
///
/// Every variant gets `Hdr`, `Msaa::Off`, `DepthPrepass` and `MotionVectorPrepass` (what DLSS
/// requires), so DLSS is the only difference between `native` and `dlss-*`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_world(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    scene: SceneKind,
    variant: &Variant,
    target: RenderTarget,
    transform: Transform,
) -> (Entity, Vec<String>) {
    let mut features: Vec<String> = vec![
        "Hdr".into(),
        "Msaa::Off".into(),
        "DepthPrepass".into(),
        "MotionVectorPrepass".into(),
        "Tonemapping::TonyMcMapface".into(),
    ];
    let camera = commands
        .spawn((
            BenchCamera,
            Camera3d::default(),
            Camera {
                order: 0,
                ..default()
            },
            target,
            Hdr,
            Msaa::Off,
            Tonemapping::TonyMcMapface,
            DepthPrepass,
            MotionVectorPrepass,
            transform,
        ))
        .id();
    if scene.uses_deferred(variant) {
        commands.entity(camera).insert(DeferredPrepass);
        features.push("DeferredPrepass".into());
    }
    #[cfg(feature = "dlss")]
    if let Some(mode) = variant.dlss {
        commands.entity(camera).insert(bevy::anti_alias::dlss::Dlss {
            perf_quality_mode: mode.to_bevy(),
            ..default()
        });
    }

    let mut ctx = SceneCtx {
        commands,
        meshes,
        materials,
        images,
        features: &mut features,
    };
    scenes::camera_extras(scene, variant, &mut ctx, camera);
    scenes::spawn_scene(scene, &mut ctx);
    (camera, features)
}

pub fn fixed_pose(scene: SceneKind) -> Transform {
    let info = scene.info();
    Transform::from_translation(info.eye).looking_at(info.target, Vec3::Y)
}

// --- Label overlay ------------------------------------------------------------------------

#[derive(Component)]
pub struct LabelLine(pub usize);

const LINE_COLORS: [Color; 7] = [
    Color::srgb(1.0, 1.0, 1.0),
    Color::srgb(0.75, 0.85, 1.0),
    Color::srgb(1.0, 0.85, 0.35),
    Color::srgb(0.7, 0.7, 0.7),
    Color::srgb(1.0, 0.5, 0.45),
    Color::srgb(0.55, 0.55, 0.55),
    Color::srgb(0.6, 0.9, 0.6),
];

pub fn label_font_size(height: u32) -> f32 {
    (height as f32 / 56.0).clamp(10.0, 30.0)
}

/// The label panel (top-left, translucent). `lines` is 6 for captures, 7 with the viewer's help line.
pub fn spawn_label_panel(parent: &mut ChildSpawnerCommands, font_size: f32, lines: usize) {
    parent
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(font_size * 0.6),
                top: Val::Px(font_size * 0.6),
                max_width: Val::Percent(62.0),
                padding: UiRect::all(Val::Px(font_size * 0.5)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(font_size * 0.15),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.72)),
        ))
        .with_children(|panel| {
            for (i, color) in LINE_COLORS.iter().take(lines).enumerate() {
                panel.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(if i == 0 {
                            font_size * 1.25
                        } else {
                            font_size
                        }),
                        ..default()
                    },
                    TextColor(*color),
                    LabelLine(i),
                ));
            }
        });
}

pub fn label_lines(meta: &Meta, variant: &Variant, frame: u32) -> Vec<String> {
    let res = |r: UVec2| format!("{}x{}", r.x, r.y);
    let render = match (meta.render, variant.dlss) {
        (Some(r), _) => res(r),
        (None, Some(mode)) => format!(
            "~{}",
            res((meta.output.as_vec2() * mode.nominal_scale()).round().as_uvec2())
        ),
        (None, None) => res(meta.output),
    };
    vec![
        format!("{}  [{} / {}]", meta.title, meta.scene, meta.variant),
        format!("Feature under test: {}", meta.feature),
        format!(
            "{}  |  render {} -> output {}  |  {}",
            variant.describe(),
            render,
            res(meta.output),
            meta.pipeline
        ),
        format!("Also on: {}", meta.features.join(", ")),
        format!("Expected: {}", meta.expected),
        format!("Bevy 0.19.1 | {} | frame {frame}", meta.adapter),
    ]
}

pub fn set_label_text(lines: &[String], query: &mut Query<(&mut Text, &LabelLine)>) {
    for (mut text, line) in query.iter_mut() {
        let new = lines.get(line.0).map(String::as_str).unwrap_or("");
        if text.0 != new {
            text.0 = new.to_owned();
        }
    }
}
