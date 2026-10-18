//! A single capture: one scene, one variant, one headless Bevy app.
//!
//! Two offscreen cameras render every frame:
//! * the 3D camera (DLSS / TAA / deferred, ...) into `raw`, the image under test;
//! * a UI-only camera into `labelled`, drawing `raw` full-size with the label on top.
//!
//! Both are captured in the same frame once all pipelines are compiled and the
//! temporal history (DLSS / TAA) had `settle` frames to converge.

use std::{path::PathBuf, time::Duration};

use bevy::{
    app::ScheduleRunnerPlugin,
    camera::RenderTarget,
    light::DirectionalLightShadowMap,
    prelude::*,
    render::{
        render_resource::{TextureFormat, TextureUsages},
        renderer::RenderAdapterInfo,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::ExitCondition,
    winit::WinitPlugin,
};

use crate::{
    meta::Meta,
    scenes::SceneKind,
    variant::Variant,
    world::{self, BenchCorePlugin, DlssAvailability, LabelLine, RenderProbe},
};

/// Process exit code of a capture whose variant needs DLSS on a machine without it.
pub const EXIT_DLSS_UNSUPPORTED: u8 = 3;
const EXIT_TIMEOUT: u8 = 4;

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

/// Hands the job to the startup system.
#[derive(Resource)]
struct JobSlot(Option<Job>);

pub fn run(job: Job) -> AppExit {
    let mut app = App::new();
    world::insert_dlss_project_id(&mut app);

    app.insert_resource(world::opaque_method(job.scene, &job.variant))
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
        .add_plugins((ScheduleRunnerPlugin::run_loop(Duration::ZERO), BenchCorePlugin))
        .insert_resource(JobSlot(Some(job)))
        .add_systems(Startup, setup)
        .add_systems(Update, drive.run_if(resource_exists::<State>));

    app.run()
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    mut slot: ResMut<JobSlot>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    adapter: Res<RenderAdapterInfo>,
    dlss: Res<DlssAvailability>,
    mut exit: MessageWriter<AppExit>,
) {
    let job = slot.0.take().expect("setup runs once");
    let mut meta = world::base_meta(job.scene, &job.variant, job.res, world::adapter_string(&adapter));

    if job.variant.dlss.is_some()
        && let Err(reason) = dlss.0
    {
        eprintln!(
            "[{}/{}] DLSS unsupported: {reason}. Adapter: {}",
            meta.scene, job.variant, meta.adapter
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

    let (_, features) = world::spawn_world(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        job.scene,
        &job.variant,
        RenderTarget::Image(raw.clone().into()),
        world::fixed_pose(job.scene),
    );

    // Label camera: raw image + overlay.
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
    let font_size = world::label_font_size(job.res.y);
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
        .with_children(|root| world::spawn_label_panel(root, font_size, 6));

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

fn drive(
    mut commands: Commands,
    mut state: ResMut<State>,
    probe: Res<RenderProbe>,
    mut lines: Query<(&mut Text, &LabelLine)>,
    mut exit: MessageWriter<AppExit>,
) {
    let data = probe.get();
    state.frame += 1;
    if state.requested {
        if state.frame > state.meta.frames + 600 {
            error!("timed out waiting for the screenshots");
            exit.write(AppExit::from_code(EXIT_TIMEOUT));
        }
        return;
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

    let text = world::label_lines(&state.meta, &state.job.variant, state.frame);
    world::set_label_text(&text, &mut lines);

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
