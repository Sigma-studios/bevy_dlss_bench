//! A single capture: one scene, one variant, one headless Bevy app.
//!
//! Two offscreen cameras render every frame:
//! * the 3D camera (DLSS / TAA / deferred, ...) into `raw`, the image under test;
//! * a UI-only camera into `labelled`, drawing `raw` full-size with the label on top.
//!
//! Both are captured in the same frame once all pipelines are compiled and the
//! temporal history (DLSS / TAA) had `settle` frames to converge.

use std::{
    fmt::Write as _,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use bevy::{
    app::ScheduleRunnerPlugin,
    camera::{Hdr, MainPassResolutionOverride, RenderTarget},
    core_pipeline::{
        prepass::{DeferredPrepass, DepthPrepass, MotionVectorPrepass},
        tonemapping::Tonemapping,
    },
    light::DirectionalLightShadowMap,
    pbr::DefaultOpaqueRendererMethod,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        render_resource::{CachedPipelineState, PipelineCache, TextureFormat, TextureUsages},
        renderer::RenderAdapterInfo,
        view::ExtractedView,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};

use crate::{
    meta::Meta,
    scenes::{self, Fence, SceneCtx, SceneKind},
    variant::Variant,
};

/// Process exit code of a capture whose variant needs DLSS on a machine without it.
pub const EXIT_DLSS_UNSUPPORTED: u8 = 3;
const EXIT_TIMEOUT: u8 = 4;

#[cfg(feature = "dlss")]
const DLSS_PROJECT_ID: bevy::asset::uuid::Uuid =
    bevy::asset::uuid::Uuid::from_u128(0x6f1c2e4a_9b3d_4e7f_8a21_5c0d9e3b7a14);

#[derive(Clone, Debug)]
pub struct Job {
    pub scene: SceneKind,
    pub variant: Variant,
    pub res: UVec2,
    pub settle: u32,
    /// `out/<scene>`
    pub scene_dir: PathBuf,
}

impl Job {
    pub fn labelled_path(&self) -> PathBuf {
        self.scene_dir.join(format!("{}.png", self.variant))
    }
    pub fn raw_path(&self) -> PathBuf {
        self.scene_dir.join("raw").join(format!("{}.png", self.variant))
    }
    pub fn meta_path(&self) -> PathBuf {
        self.scene_dir.join(format!("{}.meta", self.variant))
    }
}

/// Written by the render world, read by the main world.
#[derive(Default, Debug, Clone, Copy)]
struct RenderProbeData {
    /// Size of the 3D camera's main pass: `MainPassResolutionOverride` if present, else the viewport.
    main_pass: Option<UVec2>,
    has_override: bool,
    pipelines_pending: usize,
}

#[derive(Resource, Clone, Default)]
struct RenderProbe(Arc<Mutex<RenderProbeData>>);

#[derive(Resource)]
struct State {
    job: Job,
    raw: Handle<Image>,
    labelled: Handle<Image>,
    meta: Meta,
    frame: u32,
    quiet_frames: u32,
    requested: bool,
    saved: u8,
}

#[derive(Component)]
struct LabelLine(usize);

pub fn run(job: Job) -> AppExit {
    let mut app = App::new();

    #[cfg(feature = "dlss")]
    app.insert_resource(bevy::anti_alias::dlss::DlssProjectId(DLSS_PROJECT_ID));

    let method = if job.scene.uses_deferred(&job.variant) {
        DefaultOpaqueRendererMethod::deferred()
    } else {
        DefaultOpaqueRendererMethod::forward()
    };
    let probe = RenderProbe::default();

    app.insert_resource(method)
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(DirectionalLightShadowMap { size: 4096 })
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .disable::<WinitPlugin>(),
        )
        // Headless loop as fast as the GPU allows; nothing depends on wall-clock time.
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::ZERO))
        .insert_resource(probe.clone())
        .insert_non_send(JobSlot(Some(job)))
        .add_systems(Startup, setup)
        .add_systems(Update, drive.run_if(resource_exists::<State>));

    app.sub_app_mut(RenderApp)
        .insert_resource(probe)
        .add_systems(Render, probe_render_world.in_set(RenderSystems::Cleanup));

    app.run()
}

/// Hands the job to the startup system (keeps `Job` out of the ECS until setup).
struct JobSlot(Option<Job>);

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut slot: NonSendMut<JobSlot>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    adapter: Res<RenderAdapterInfo>,
    #[cfg(feature = "dlss")] dlss_supported: Option<
        Res<bevy::anti_alias::dlss::DlssSuperResolutionSupported>,
    >,
    mut exit: MessageWriter<AppExit>,
) {
    let job = slot.0.take().expect("setup runs once");
    let info = job.scene.info();
    let deferred = job.scene.uses_deferred(&job.variant);

    #[cfg(feature = "dlss")]
    let dlss_unavailable = dlss_supported
        .is_none()
        .then_some("DLSS Super Resolution not supported by this GPU / driver / backend");
    #[cfg(not(feature = "dlss"))]
    let dlss_unavailable = Some("binary built without the `dlss` cargo feature");

    let mut meta = Meta {
        status: "ok".into(),
        scene: info.name.into(),
        title: info.title.into(),
        feature: info.feature.into(),
        variant: job.variant.name(),
        variant_desc: job.variant.describe(),
        expected: job.scene.expected_for(&job.variant).into(),
        pipeline: if deferred { "deferred" } else { "forward" }.into(),
        output: job.res,
        render: None,
        adapter: if adapter.driver_info.is_empty() {
            format!("{} ({:?})", adapter.name, adapter.backend)
        } else {
            format!("{} ({:?}, {})", adapter.name, adapter.backend, adapter.driver_info)
        },
        features: Vec::new(),
        frames: 0,
    };

    if job.variant.dlss.is_some()
        && let Some(reason) = dlss_unavailable
    {
        eprintln!(
            "[{}/{}] DLSS unsupported: {reason}. Adapter: {}",
            info.name, job.variant, meta.adapter
        );
        meta.status = format!("dlss-unsupported: {reason}");
        meta.write(&job.meta_path());
        exit.write(AppExit::from_code(EXIT_DLSS_UNSUPPORTED));
        return;
    }

    let target = |images: &mut Assets<Image>| {
        let mut image =
            Image::new_target_texture(job.res.x, job.res.y, TextureFormat::Rgba8UnormSrgb, None);
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        images.add(image)
    };
    let raw = target(&mut images);
    let labelled = target(&mut images);

    // --- 3D camera under test -------------------------------------------------------------
    let mut features: Vec<String> = vec![
        "Hdr".into(),
        "Msaa::Off".into(),
        "DepthPrepass".into(),
        "MotionVectorPrepass".into(),
        "Tonemapping::TonyMcMapface".into(),
    ];
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera {
                order: 0,
                ..default()
            },
            RenderTarget::Image(raw.clone().into()),
            Hdr,
            Msaa::Off,
            Tonemapping::TonyMcMapface,
            // Native shots get the same prepasses DLSS requires, so DLSS is the only difference.
            DepthPrepass,
            MotionVectorPrepass,
            Transform::from_translation(info.eye).looking_at(info.target, Vec3::Y),
        ))
        .id();
    if deferred {
        commands.entity(camera).insert(DeferredPrepass);
        features.push("DeferredPrepass".into());
    }
    #[cfg(feature = "dlss")]
    if let Some(mode) = job.variant.dlss {
        commands.entity(camera).insert(bevy::anti_alias::dlss::Dlss {
            perf_quality_mode: mode.to_bevy(),
            ..default()
        });
    }

    let mut ctx = SceneCtx {
        commands: &mut commands,
        meshes: &mut meshes,
        materials: &mut materials,
        images: &mut images,
        features: &mut features,
    };
    scenes::camera_extras(job.scene, &job.variant, &mut ctx, camera);
    scenes::spawn_scene(job.scene, &mut ctx);

    // --- Label camera: raw image + overlay ----------------------------------------------
    let ui_camera = commands
        .spawn((
            Camera2d,
            Camera {
                order: 1,
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
            RenderTarget::Image(labelled.clone().into()),
        ))
        .id();
    let font_size = (job.res.y as f32 / 56.0).clamp(10.0, 30.0);
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ImageNode::new(raw.clone()),
            UiTargetCamera(ui_camera),
        ))
        .with_children(|root| {
            root.spawn((
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
                for (i, color) in LINE_COLORS.iter().enumerate() {
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
        });

    meta.features = features;
    commands.insert_resource(State {
        job,
        raw,
        labelled,
        meta,
        frame: 0,
        quiet_frames: 0,
        requested: false,
        saved: 0,
    });
}

const LINE_COLORS: [Color; 6] = [
    Color::srgb(1.0, 1.0, 1.0),
    Color::srgb(0.75, 0.85, 1.0),
    Color::srgb(1.0, 0.85, 0.35),
    Color::srgb(0.7, 0.7, 0.7),
    Color::srgb(1.0, 0.5, 0.45),
    Color::srgb(0.55, 0.55, 0.55),
];

fn label_lines(state: &State) -> [String; 6] {
    let m = &state.meta;
    let v = &state.job.variant;
    let res = |r: UVec2| format!("{}x{}", r.x, r.y);
    let render = match (m.render, v.dlss) {
        (Some(r), _) => res(r),
        (None, Some(mode)) => format!(
            "~{}",
            res((m.output.as_vec2() * mode.nominal_scale()).round().as_uvec2())
        ),
        (None, None) => res(m.output),
    };
    let mut features = String::new();
    for (i, f) in m.features.iter().enumerate() {
        let _ = write!(features, "{}{f}", if i == 0 { "" } else { ", " });
    }
    [
        format!("{}  [{} / {}]", m.title, m.scene, m.variant),
        format!("Feature under test: {}", m.feature),
        format!(
            "{}  |  render {} -> output {}  |  {}",
            v.describe(),
            render,
            res(m.output),
            m.pipeline
        ),
        format!("Also on: {features}"),
        format!("Expected: {}", m.expected),
        format!(
            "Bevy 0.19.1 | {} | frame {}",
            m.adapter, state.frame
        ),
    ]
}

fn drive(
    mut commands: Commands,
    mut state: ResMut<State>,
    probe: Res<RenderProbe>,
    mut fences: Query<&mut Transform, With<Fence>>,
    mut lines: Query<(&mut Text, &LabelLine)>,
    mut exit: MessageWriter<AppExit>,
) {
    let data = *probe.0.lock().unwrap();
    if state.requested {
        state.frame += 1;
        if state.frame > state.job.settle + 600 {
            error!("timed out waiting for the screenshots");
            exit.write(AppExit::from_code(EXIT_TIMEOUT));
        }
        return;
    }

    state.frame += 1;
    if state.job.scene.info().temporal {
        scenes::animate_fence(state.frame, &mut fences);
    }
    if data.has_override || state.job.variant.dlss.is_none() {
        state.meta.render = data.main_pass;
    }

    // Count frames only once every pipeline is compiled, so history has really converged.
    if data.main_pass.is_some() && data.pipelines_pending == 0 {
        state.quiet_frames += 1;
    } else {
        state.quiet_frames = 0;
    }
    if state.frame > 20_000 {
        error!("pipelines never settled ({} pending)", data.pipelines_pending);
        exit.write(AppExit::from_code(EXIT_TIMEOUT));
        return;
    }

    let text = label_lines(&state);
    for (mut t, line) in &mut lines {
        if t.0 != text[line.0] {
            t.0.clone_from(&text[line.0]);
        }
    }

    if state.quiet_frames < state.job.settle {
        return;
    }
    if state.job.variant.dlss.is_some() && !data.has_override {
        warn!("DLSS variant but no MainPassResolutionOverride on the camera: DLSS is not active");
        state.meta.status = "dlss-inactive (no MainPassResolutionOverride)".into();
    }
    state.meta.frames = state.frame;
    state.requested = true;

    for (handle, path) in [
        (state.raw.clone(), state.job.raw_path()),
        (state.labelled.clone(), state.job.labelled_path()),
    ] {
        commands.spawn(Screenshot::image(handle)).observe(
            move |captured: On<ScreenshotCaptured>,
                  mut state: ResMut<State>,
                  mut exit: MessageWriter<AppExit>| {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match captured.image.clone().try_into_dynamic() {
                    Ok(img) => {
                        if let Err(e) = img.to_rgb8().save(&path) {
                            error!("cannot save {}: {e}", path.display());
                        }
                    }
                    Err(e) => error!("cannot convert screenshot: {e:?}"),
                }
                state.saved += 1;
                if state.saved == 2 {
                    state.meta.write(&state.job.meta_path());
                    println!(
                        "[{}/{}] saved {}",
                        state.meta.scene,
                        state.meta.variant,
                        state.job.labelled_path().display()
                    );
                    exit.write(AppExit::Success);
                }
            },
        );
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
    if let Some((view, resolution_override)) = views.iter().next() {
        data.has_override = resolution_override.is_some();
        data.main_pass = Some(
            resolution_override
                .map(|o| o.0)
                .unwrap_or(view.viewport.zw()),
        );
    }
}
