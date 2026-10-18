//! Interactive viewer: the same scenes and variants as the captures, in a window, with live
//! switching. Useful to step through a bug or check a fix against a patched Bevy.
//!
//! Any change of scene or variant rebuilds the scene (the renderer method is baked into the
//! materials when they are prepared), keeping the camera pose unless the scene changes.

use std::path::PathBuf;

use bevy::{
    camera::RenderTarget,
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    light::DirectionalLightShadowMap,
    pbr::DefaultOpaqueRendererMethod,
    prelude::*,
    render::{
        renderer::RenderAdapterInfo,
        view::screenshot::{Screenshot, save_to_disk},
    },
    window::{PresentMode, WindowResolution},
};

use crate::{
    meta::Meta,
    scenes::{self, Fence, SceneKind},
    variant::{DlssMode, Variant},
    world::{self, BenchCamera, BenchCorePlugin, DlssAvailability, LabelLine, RenderProbe},
};

const HELP: &str = "1-6 / [ ] scene | Z DLSS mode | R reset history | \
                    C camera reset | H label | P screenshot | \
                    fly: right-drag + WASD/QE (Shift fast) | Esc quit";

/// Order of the DLSS cycle bound to `Z`.
const DLSS_CYCLE: [Option<DlssMode>; 6] = [
    None,
    Some(DlssMode::Quality),
    Some(DlssMode::Balanced),
    Some(DlssMode::Performance),
    Some(DlssMode::UltraPerformance),
    Some(DlssMode::Dlaa),
];

#[derive(Resource)]
struct Viewer {
    scene: SceneKind,
    variant: Variant,
    res: UVec2,
    out: PathBuf,
    rebuild: bool,
    /// Go back to the scene's fixed camera on the next rebuild (else keep the free camera
    /// where the user put it, so variants are compared from the same view).
    reset_pose: bool,
    camera: Option<Entity>,
    label: bool,
    /// Frames since start (label, message timeouts).
    ticks: u32,
    shots: u32,
    meta: Meta,
    /// Transient message shown in the help line (e.g. "saved ...").
    message: Option<(String, u32)>,
}

/// Root of the label overlay.
#[derive(Component)]
struct LabelRoot;

pub fn run(scene: SceneKind, variant: Variant, res: UVec2, out: PathBuf) -> AppExit {
    let mut app = App::new();
    world::insert_dlss_project_id(&mut app);
    app.insert_resource(world::opaque_method(scene, &variant))
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(DirectionalLightShadowMap { size: 4096 })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_dlss_bench viewer".into(),
                // Physical size = --res, regardless of the monitor scale factor.
                resolution: WindowResolution::new(res.x, res.y).with_scale_factor_override(1.0),
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((BenchCorePlugin, FreeCameraPlugin))
        .insert_resource(Viewer {
            scene,
            variant,
            res,
            out,
            rebuild: true,
            reset_pose: true,
            camera: None,
            label: true,
            ticks: 0,
            shots: 0,
            meta: Meta::default(),
            message: None,
        })
        .add_systems(Update, (input, rebuild, tick, update_label).chain());
    app.run()
}

fn input(
    keys: Res<ButtonInput<KeyCode>>,
    mut viewer: ResMut<Viewer>,
    dlss: Res<DlssAvailability>,
    mut commands: Commands,
    mut cameras: Query<&mut Transform, With<BenchCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    let v = &mut *viewer;
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
    ];
    let current = SceneKind::ALL
        .iter()
        .position(|s| *s == v.scene)
        .unwrap_or(0);
    let mut next_scene = None;
    for (i, key) in digits.iter().enumerate() {
        if keys.just_pressed(*key) && i < SceneKind::ALL.len() {
            next_scene = Some(i);
        }
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        next_scene = Some((current + 1) % SceneKind::ALL.len());
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        next_scene = Some((current + SceneKind::ALL.len() - 1) % SceneKind::ALL.len());
    }
    if let Some(i) = next_scene
        && SceneKind::ALL[i] != v.scene
    {
        v.scene = SceneKind::ALL[i];
        v.reset_pose = true;
        if !v.scene.info().deferred {
            v.variant.forward = false;
        }
        v.rebuild = true;
    }

    if keys.just_pressed(KeyCode::KeyZ) {
        match dlss.0 {
            Ok(()) => {
                let i = DLSS_CYCLE
                    .iter()
                    .position(|m| *m == v.variant.dlss)
                    .unwrap_or(0);
                v.variant.dlss = DLSS_CYCLE[(i + 1) % DLSS_CYCLE.len()];
                v.rebuild = true;
            }
            Err(reason) => v.message = Some((format!("DLSS unavailable: {reason}"), v.ticks + 240)),
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        if let Some(camera) = v.camera {
            #[cfg(feature = "dlss")]
            commands.entity(camera).queue(|mut e: EntityWorldMut| {
                if let Some(mut d) = e.get_mut::<bevy::anti_alias::dlss::Dlss>() {
                    d.reset = true;
                }
            });
            commands.entity(camera).queue(|mut e: EntityWorldMut| {
                if let Some(mut t) = e.get_mut::<bevy::anti_alias::taa::TemporalAntiAliasing>() {
                    t.reset = true;
                }
            });
        }
        v.message = Some(("history reset".into(), v.ticks + 120));
    }
    if keys.just_pressed(KeyCode::KeyH) {
        v.label = !v.label;
    }
    if keys.just_pressed(KeyCode::KeyC) {
        for mut t in &mut cameras {
            *t = world::fixed_pose(v.scene);
        }
    }
    if keys.just_pressed(KeyCode::KeyP) {
        v.shots += 1;
        let path = v
            .out
            .join("view")
            .join(v.scene.info().name)
            .join(format!("{}-{:03}.png", v.variant, v.shots));
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        v.message = Some((format!("saved {}", path.display()), v.ticks + 240));
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

/// Everything a scene spawns, plus the camera and the label.
type SceneEntities = Or<(
    With<Mesh3d>,
    With<PointLight>,
    With<DirectionalLight>,
    With<SpotLight>,
    With<BenchCamera>,
    With<Fence>,
    With<LabelRoot>,
)>;

#[allow(clippy::too_many_arguments)]
fn rebuild(
    mut commands: Commands,
    mut viewer: ResMut<Viewer>,
    mut method: ResMut<DefaultOpaqueRendererMethod>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    adapter: Res<RenderAdapterInfo>,
    cameras: Query<&Transform, With<BenchCamera>>,
    old: Query<Entity, SceneEntities>,
) {
    if !viewer.rebuild {
        return;
    }
    let v = &mut *viewer;
    v.rebuild = false;
    let kept = v.camera.and_then(|c| cameras.get(c).ok().copied());
    let pose = match kept {
        Some(t) if !v.reset_pose => t,
        _ => world::fixed_pose(v.scene),
    };
    v.reset_pose = false;
    // TAA only exists in the temporal scene, where it is always on.
    v.variant.taa = v.scene.info().temporal;
    for e in &old {
        commands.entity(e).try_despawn();
    }

    *method = world::opaque_method(v.scene, &v.variant);
    let (camera, features) = world::spawn_world(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        v.scene,
        &v.variant,
        RenderTarget::default(),
        pose,
    );
    commands.entity(camera).insert(FreeCamera::default());

    let font_size = world::label_font_size(v.res.y);
    commands
        .spawn((
            LabelRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            UiTargetCamera(camera),
        ))
        .with_children(|root| world::spawn_label_panel(root, font_size, 7));

    v.camera = Some(camera);
    v.meta = world::base_meta(v.scene, &v.variant, v.res, world::adapter_string(&adapter));
    v.meta.features = features;
    info!("viewer: {} / {}", v.scene.info().name, v.variant);
}

fn tick(mut viewer: ResMut<Viewer>, mut fences: Query<&mut Transform, With<Fence>>) {
    viewer.ticks += 1;
    if viewer.scene.info().temporal {
        scenes::animate_fence(viewer.ticks, &mut fences);
    }
}

fn update_label(
    mut viewer: ResMut<Viewer>,
    probe: Res<RenderProbe>,
    windows: Query<&Window>,
    mut roots: Query<&mut Visibility, With<LabelRoot>>,
    mut lines: Query<(&mut Text, &LabelLine)>,
) {
    let v = &mut *viewer;
    // The window may have been resized: report the real output size.
    if let Ok(window) = windows.single() {
        v.meta.output = UVec2::new(window.physical_width(), window.physical_height());
    }
    let data = probe.get();
    v.meta.render = if data.has_override || v.variant.dlss.is_none() {
        data.main_pass
    } else {
        None
    };
    for mut vis in &mut roots {
        vis.set_if_neq(if v.label {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    let mut text = world::label_lines(&v.meta, &v.variant, v.ticks);
    if let Some((_, until)) = &v.message
        && v.ticks > *until
    {
        v.message = None;
    }
    text.push(match &v.message {
        Some((msg, _)) => msg.clone(),
        None => HELP.to_owned(),
    });
    world::set_label_text(&text, &mut lines);
}
