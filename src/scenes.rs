//! One minimal, deterministic scene per rendering feature under test.
//!
//! Each scene isolates a single feature: everything that is not needed to show the
//! artefact is off (no shadows, SSAO, fog, ... unless that is the feature being tested).

use std::f32::consts::{FRAC_PI_2, PI};

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    light::{CascadeShadowConfigBuilder, NotShadowCaster},
    math::Affine2,
    pbr::{DistanceFog, FogFalloff, ScreenSpaceAmbientOcclusion, ScreenSpaceReflections},
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
    },
};

use crate::variant::{DlssMode, Variant};

/// Fixed seed for every pseudo-random choice in the scenes.
const SEED: u64 = 0x5EED_D155_B3AC_0001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneKind {
    DeferredLighting,
    ClusteredLights,
    Ssr,
    Ssao,
    Fog,
    Skybox,
    Taa,
}

pub struct SceneInfo {
    pub name: &'static str,
    pub title: &'static str,
    /// The Bevy feature this scene isolates.
    pub feature: &'static str,
    /// One-line caption of the artefact expected under DLSS.
    pub expected: &'static str,
    /// Caption for the `dlss-*-forward` variant (only for deferred scenes).
    pub expected_forward: &'static str,
    /// Caption for the `dlss-*-taa` variant (only for the TAA scene).
    pub expected_taa: &'static str,
    /// Scene renders opaque geometry through the deferred path by default.
    pub deferred: bool,
    /// A `dlss-*-forward` variant makes sense (feature also exists in forward).
    pub forward_variant: bool,
    /// Scene tests temporal artefacts (adds TAA variants and a deterministic animation).
    pub temporal: bool,
    pub eye: Vec3,
    pub target: Vec3,
}

impl SceneKind {
    pub const ALL: [SceneKind; 7] = [
        SceneKind::DeferredLighting,
        SceneKind::ClusteredLights,
        SceneKind::Ssr,
        SceneKind::Ssao,
        SceneKind::Fog,
        SceneKind::Skybox,
        SceneKind::Taa,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.info().name == s)
    }

    pub fn info(self) -> SceneInfo {
        let base = SceneInfo {
            name: "",
            title: "",
            feature: "",
            expected: "",
            expected_forward: "",
            expected_taa: "",
            deferred: false,
            forward_variant: false,
            temporal: false,
            eye: Vec3::new(7.0, 5.5, 9.0),
            target: Vec3::new(0.0, 0.5, 0.0),
        };
        match self {
            SceneKind::DeferredLighting => SceneInfo {
                name: "deferred_lighting",
                title: "1. Deferred lighting",
                feature: "deferred lighting pass: world position rebuilt by frag_coord_to_ndc() from view.viewport",
                expected: "lighting & shadows are computed for the wrong world position: shadows shift/stretch, \
                           lit area shrinks toward the top-left",
                expected_forward: "forward shading uses the interpolated world position: shadows match native",
                deferred: true,
                forward_variant: true,
                ..base
            },
            SceneKind::ClusteredLights => SceneInfo {
                name: "clustered_lights",
                title: "2. Clustered point lights",
                feature: "light clustering: tiles sized from the full camera viewport (cluster_factors)",
                expected: "light pools are cut along cluster tile edges / missing, and do not sit under their \
                           marker spheres",
                eye: Vec3::new(0.0, 10.0, 11.0),
                target: Vec3::new(0.0, 0.0, -0.5),
                ..base
            },
            SceneKind::Ssr => SceneInfo {
                name: "ssr",
                title: "3. Screen-space reflections",
                feature: "SSR: ray origin from frag_coord_to_ndc() and full-size depth UVs",
                expected: "reflections are offset/scaled relative to the objects, smeared or missing \
                           (deferred lighting artefacts also present: SSR requires deferred)",
                deferred: true,
                eye: Vec3::new(0.0, 1.1, 4.2),
                target: Vec3::new(0.0, 0.4, -1.5),
                ..base
            },
            SceneKind::Ssao => SceneInfo {
                name: "ssao",
                title: "4. SSAO",
                feature: "SSAO compute passes dispatched and sampled with camera.physical_viewport_size",
                expected: "contact occlusion misplaced, stretched or clipped: AO no longer hugs the box/wall \
                           creases",
                eye: Vec3::new(3.8, 3.2, 4.8),
                target: Vec3::new(-1.2, 0.6, -1.2),
                ..base
            },
            SceneKind::Fog => SceneInfo {
                name: "fog",
                title: "5. Distance fog (deferred)",
                feature: "DistanceFog applied in the deferred lighting pass from the reconstructed world position",
                expected: "fog density no longer follows depth: near pillars fogged, far ones too clear, \
                           fog gradient bends toward the top-left",
                expected_forward: "forward fog uses the mesh world position: matches native",
                deferred: true,
                forward_variant: true,
                eye: Vec3::new(0.0, 1.7, 6.0),
                target: Vec3::new(0.6, 1.4, -20.0),
                ..base
            },
            SceneKind::Skybox => SceneInfo {
                name: "skybox",
                title: "6. Skybox (control: fixed in 0.20)",
                feature: "skybox ray direction from view.viewport (bevy#25604 switches to main_pass_viewport)",
                expected: "sky is zoomed/offset relative to the geometry: the cubemap seam no longer lines up \
                           with the red marker post",
                eye: Vec3::new(0.0, 1.4, 0.0),
                target: Vec3::new(-1.0, 1.55, -1.0),
                ..base
            },
            SceneKind::Taa => SceneInfo {
                name: "taa",
                title: "7. TAA + DLSS (double jitter)",
                feature: "TemporalJitter written by both prepare_taa_jitter and prepare_dlss; TAA resolves DLSS output",
                expected: "control: DLSS without TAA, thin wires stable",
                expected_taa: "two jitter sequences fight over TemporalJitter and TAA re-resolves the upscaled \
                               image: wires shimmer, ghost or blur",
                temporal: true,
                eye: Vec3::new(0.4, 1.2, 4.2),
                target: Vec3::new(0.0, 1.0, 0.0),
                ..base
            },
        }
    }

    pub fn variants(self, modes: &[DlssMode]) -> Vec<Variant> {
        let info = self.info();
        // The temporal scene runs with TAA on by default; DLSS without TAA is its control.
        let taa = info.temporal;
        let mut out = vec![Variant {
            taa,
            ..Variant::NATIVE
        }];
        for &mode in modes {
            out.push(Variant {
                dlss: Some(mode),
                taa,
                ..Variant::NATIVE
            });
        }
        if let Some(&first) = modes.first() {
            if info.deferred && info.forward_variant {
                out.push(Variant {
                    dlss: Some(first),
                    forward: true,
                    taa: false,
                });
            }
            if info.temporal {
                out.push(Variant {
                    dlss: Some(first),
                    ..Variant::NATIVE
                });
            }
        }
        out
    }

    /// Whether this variant renders opaque geometry through the deferred path.
    pub fn uses_deferred(self, variant: &Variant) -> bool {
        self.info().deferred && !variant.forward
    }

    pub fn expected_for(self, variant: &Variant) -> &'static str {
        let info = self.info();
        match (variant.dlss, variant.forward, variant.taa) {
            (None, _, false) => "reference image: no artefact expected",
            (None, _, true) => "reference image (TAA only): no artefact expected",
            (Some(_), true, _) => info.expected_forward,
            (Some(_), false, true) => info.expected_taa,
            (Some(_), false, false) => info.expected,
        }
    }
}

/// Small deterministic PRNG (SplitMix64) so "random" scene content is identical across runs.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Marks the fence of the TAA scene, moved by a frame-count-driven (deterministic) animation.
#[derive(Component)]
pub struct Fence;

/// Everything a scene needs to spawn itself.
pub struct SceneCtx<'a, 'w, 's> {
    pub commands: &'a mut Commands<'w, 's>,
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
    pub images: &'a mut Assets<Image>,
    /// Human-readable list of the features this scene turns on (shown in the label).
    pub features: &'a mut Vec<String>,
}

impl SceneCtx<'_, '_, '_> {
    fn mat(&mut self, color: Color, roughness: f32) -> Handle<StandardMaterial> {
        self.materials.add(StandardMaterial {
            base_color: color,
            perceptual_roughness: roughness,
            ..default()
        })
    }

    fn spawn_mesh(&mut self, mesh: impl Into<Mesh>, material: Handle<StandardMaterial>, t: Transform) {
        let mesh = self.meshes.add(mesh);
        self.commands
            .spawn((Mesh3d(mesh), MeshMaterial3d(material), t));
    }

    fn checker_floor(&mut self, size: Vec2, tile: f32, a: [u8; 3], b: [u8; 3]) {
        let image = self.images.add(checker_image(a, b));
        let material = self.materials.add(StandardMaterial {
            base_color_texture: Some(image),
            perceptual_roughness: 0.9,
            // The texture holds 2x2 cells, so one repeat covers 2 tiles per axis.
            uv_transform: Affine2::from_scale(size / (2.0 * tile)),
            ..default()
        });
        let mesh = self.meshes.add(Plane3d::default().mesh().size(size.x, size.y));
        self.commands
            .spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::IDENTITY));
    }

    fn ambient(&mut self, brightness: f32) {
        self.commands.insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness,
            ..default()
        });
        if brightness > 0.0 {
            self.features.push(format!("ambient {brightness} cd/m2"));
        }
    }

    fn sun(&mut self, illuminance: f32, dir: Vec3, shadows: bool) {
        let mut e = self.commands.spawn((
            DirectionalLight {
                illuminance,
                shadow_maps_enabled: shadows,
                ..default()
            },
            Transform::default().looking_to(dir, Vec3::Y),
        ));
        if shadows {
            e.insert(
                CascadeShadowConfigBuilder {
                    num_cascades: 1,
                    minimum_distance: 0.1,
                    maximum_distance: 30.0,
                    ..default()
                }
                .build(),
            );
            self.features
                .push("directional light + shadows (1 cascade, 4096^2)".into());
        } else {
            self.features.push("directional light (no shadows)".into());
        }
    }

    fn point_light_grid(&mut self, cols: u32, rows: u32, spacing: f32, origin: Vec3) {
        let marker = self.meshes.add(Sphere::new(0.07).mesh().ico(2).unwrap());
        let mut rng = Rng::new(SEED);
        for r in 0..rows {
            for c in 0..cols {
                let pos = origin
                    + Vec3::new(
                        (c as f32 - (cols - 1) as f32 * 0.5) * spacing,
                        0.0,
                        (r as f32 - (rows - 1) as f32 * 0.5) * spacing,
                    );
                let color = Color::hsl(rng.next_f32() * 360.0, 0.85, 0.6);
                let material = self.materials.add(StandardMaterial {
                    base_color: color,
                    unlit: true,
                    ..default()
                });
                self.commands.spawn((
                    PointLight {
                        color,
                        intensity: 12_000.0,
                        range: 2.4,
                        radius: 0.05,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    Transform::from_translation(pos),
                ));
                self.commands.spawn((
                    Mesh3d(marker.clone()),
                    MeshMaterial3d(material),
                    Transform::from_translation(pos),
                    NotShadowCaster,
                ));
            }
        }
        self.features.push(format!(
            "{} clustered point lights (no shadows)",
            cols * rows
        ));
    }

    /// Boxes, a sphere and a thin pole on a checker floor: long, readable shadows.
    fn shadow_props(&mut self) {
        self.checker_floor(Vec2::splat(24.0), 1.0, [150, 150, 150], [95, 95, 95]);
        let red = self.mat(Color::srgb(0.8, 0.15, 0.1), 0.6);
        let blue = self.mat(Color::srgb(0.1, 0.3, 0.85), 0.6);
        let yellow = self.mat(Color::srgb(0.9, 0.75, 0.1), 0.6);
        let white = self.mat(Color::srgb(0.85, 0.85, 0.85), 0.7);
        self.spawn_mesh(
            Cuboid::new(1.2, 1.2, 1.2),
            red,
            Transform::from_xyz(-2.5, 0.6, 0.0),
        );
        self.spawn_mesh(
            Cuboid::new(1.5, 2.0, 1.5),
            blue,
            Transform::from_xyz(1.5, 1.0, -2.0),
        );
        self.spawn_mesh(
            Sphere::new(0.7).mesh().uv(48, 24),
            yellow,
            Transform::from_xyz(0.2, 0.7, 1.8),
        );
        self.spawn_mesh(
            Cuboid::new(0.15, 3.5, 0.15),
            white.clone(),
            Transform::from_xyz(3.0, 1.75, 1.5),
        );
        self.spawn_mesh(
            Cuboid::new(5.0, 2.2, 0.25),
            white,
            Transform::from_xyz(-1.0, 1.1, -4.5),
        );
    }
}

pub fn spawn_scene(kind: SceneKind, ctx: &mut SceneCtx) {
    match kind {
        SceneKind::DeferredLighting => {
            ctx.ambient(150.0);
            ctx.sun(8_000.0, Vec3::new(-1.0, -1.1, -0.55), true);
            ctx.shadow_props();
        }
        SceneKind::ClusteredLights => {
            ctx.ambient(0.0);
            let floor = ctx.mat(Color::srgb(0.55, 0.55, 0.55), 0.85);
            ctx.spawn_mesh(
                Plane3d::default().mesh().size(26.0, 18.0),
                floor,
                Transform::IDENTITY,
            );
            ctx.point_light_grid(9, 6, 2.4, Vec3::new(0.0, 0.45, -1.0));
        }
        SceneKind::Ssr => {
            ctx.ambient(250.0);
            ctx.sun(6_000.0, Vec3::new(-0.4, -1.0, -0.8), false);
            let floor = ctx.materials.add(StandardMaterial {
                base_color: Color::srgb(0.12, 0.13, 0.16),
                // SSR only runs between min_perceptual_roughness (0.08..0.12) and
                // max_perceptual_roughness (0.55..0.6).
                perceptual_roughness: 0.15,
                metallic: 0.6,
                reflectance: 0.9,
                ..default()
            });
            ctx.spawn_mesh(
                Plane3d::default().mesh().size(30.0, 30.0),
                floor,
                Transform::IDENTITY,
            );
            let colors = [
                Color::srgb(0.9, 0.1, 0.1),
                Color::srgb(0.1, 0.8, 0.2),
                Color::srgb(0.15, 0.3, 0.95),
                Color::srgb(0.95, 0.8, 0.1),
                Color::srgb(0.85, 0.2, 0.85),
            ];
            for (i, color) in colors.into_iter().enumerate() {
                let x = (i as f32 - 2.0) * 1.5;
                let m = ctx.mat(color, 0.5);
                if i % 2 == 0 {
                    let h = 0.8 + i as f32 * 0.3;
                    ctx.spawn_mesh(
                        Cuboid::new(0.8, h, 0.8),
                        m,
                        Transform::from_xyz(x, h * 0.5, -2.0)
                            .with_rotation(Quat::from_rotation_y(0.4)),
                    );
                } else {
                    ctx.spawn_mesh(
                        Sphere::new(0.5).mesh().uv(48, 24),
                        m,
                        Transform::from_xyz(x, 0.5, -1.2),
                    );
                }
            }
        }
        SceneKind::Ssao => {
            // Ambient only: SSAO only attenuates ambient/indirect light.
            ctx.ambient(1_500.0);
            let grey = ctx.mat(Color::srgb(0.8, 0.8, 0.8), 0.9);
            let warm = ctx.mat(Color::srgb(0.85, 0.65, 0.45), 0.9);
            // Floor and two walls forming a corner at (-3, 0, -3).
            ctx.spawn_mesh(
                Cuboid::new(20.0, 0.1, 20.0),
                grey.clone(),
                Transform::from_xyz(6.9, -0.05, 6.9),
            );
            ctx.spawn_mesh(
                Cuboid::new(20.0, 4.0, 0.1),
                grey.clone(),
                Transform::from_xyz(6.9, 2.0, -3.05),
            );
            ctx.spawn_mesh(
                Cuboid::new(0.1, 4.0, 20.0),
                grey.clone(),
                Transform::from_xyz(-3.05, 2.0, 6.9),
            );
            // Boxes pushed into the corner and against the walls, plus a stack.
            ctx.spawn_mesh(Cuboid::new(1.0, 1.0, 1.0), warm.clone(), Transform::from_xyz(-2.5, 0.5, -2.5));
            ctx.spawn_mesh(Cuboid::new(0.6, 0.6, 0.6), warm.clone(), Transform::from_xyz(-2.5, 1.3, -2.5));
            ctx.spawn_mesh(Cuboid::new(1.2, 0.5, 0.7), warm.clone(), Transform::from_xyz(0.2, 0.25, -2.65));
            ctx.spawn_mesh(Cuboid::new(0.5, 1.6, 0.5), warm.clone(), Transform::from_xyz(-2.75, 0.8, 0.6));
            ctx.spawn_mesh(Cuboid::new(0.5, 0.5, 0.5), warm.clone(), Transform::from_xyz(1.6, 0.25, 0.4));
            ctx.spawn_mesh(Cuboid::new(0.5, 0.5, 0.5), warm.clone(), Transform::from_xyz(2.1, 0.25, 0.4));
            ctx.spawn_mesh(Cuboid::new(0.5, 0.5, 0.5), warm, Transform::from_xyz(1.85, 0.75, 0.4));
            ctx.spawn_mesh(
                Sphere::new(0.45).mesh().uv(48, 24),
                grey,
                Transform::from_xyz(-0.9, 0.45, -0.6),
            );
        }
        SceneKind::Fog => {
            ctx.ambient(400.0);
            ctx.sun(5_000.0, Vec3::new(0.3, -1.0, -0.4), false);
            ctx.checker_floor(Vec2::new(8.0, 90.0), 1.0, [140, 140, 140], [100, 100, 100]);
            let pillar = ctx.mat(Color::srgb(0.85, 0.82, 0.75), 0.8);
            let mesh = ctx.meshes.add(Cuboid::new(0.5, 4.0, 0.5));
            for i in 0..16 {
                let z = 2.0 - i as f32 * 3.5;
                for x in [-2.2, 2.2] {
                    ctx.commands.spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(pillar.clone()),
                        Transform::from_xyz(x, 2.0, z),
                    ));
                }
            }
        }
        SceneKind::Skybox => {
            ctx.ambient(500.0);
            ctx.sun(6_000.0, Vec3::new(0.3, -1.0, -0.2), false);
            let disk = ctx.mat(Color::srgb(0.3, 0.3, 0.3), 0.9);
            ctx.spawn_mesh(Circle::new(6.0), disk, Transform::from_rotation(Quat::from_rotation_x(-FRAC_PI_2)));
            // Posts on a ring, the red one exactly on the -X/-Z cube edge (the face seam).
            let white = ctx.mat(Color::srgb(0.9, 0.9, 0.9), 0.6);
            let red = ctx.mat(Color::srgb(0.95, 0.05, 0.05), 0.6);
            for i in 0..16 {
                let a = i as f32 / 16.0 * 2.0 * PI;
                let dir = Vec3::new(a.cos(), 0.0, a.sin());
                let m = if i == 10 { red.clone() } else { white.clone() };
                ctx.spawn_mesh(
                    Cuboid::new(0.12, 2.6, 0.12),
                    m,
                    Transform::from_translation(dir * 5.0 + Vec3::Y * 1.3),
                );
            }
        }
        SceneKind::Taa => {
            ctx.ambient(600.0);
            ctx.sun(7_000.0, Vec3::new(-0.3, -1.0, -0.6), false);
            // High-frequency backdrop and floor (no mipmaps): worst case for shimmer.
            let checker = ctx.images.add(checker_image([230, 230, 230], [25, 25, 25]));
            let fine = ctx.materials.add(StandardMaterial {
                base_color_texture: Some(checker),
                perceptual_roughness: 0.9,
                uv_transform: Affine2::from_scale(Vec2::new(40.0, 12.0)),
                ..default()
            });
            ctx.spawn_mesh(
                Plane3d::default().mesh().size(16.0, 5.0),
                fine.clone(),
                Transform::from_xyz(0.0, 2.0, -3.0).with_rotation(Quat::from_rotation_x(FRAC_PI_2)),
            );
            ctx.spawn_mesh(Plane3d::default().mesh().size(16.0, 8.0), fine, Transform::IDENTITY);
            // Chain-link style fence of thin wires.
            let wire = ctx.mat(Color::srgb(0.95, 0.85, 0.3), 0.4);
            let vertical = ctx.meshes.add(Cylinder::new(0.006, 2.0));
            let horizontal = ctx.meshes.add(Cylinder::new(0.006, 6.0));
            ctx.commands
                .spawn((Fence, Transform::default(), Visibility::default()))
                .with_children(|fence| {
                    for i in 0..=60 {
                        fence.spawn((
                            Mesh3d(vertical.clone()),
                            MeshMaterial3d(wire.clone()),
                            Transform::from_xyz(-3.0 + i as f32 * 0.1, 1.0, 0.0),
                        ));
                    }
                    for j in 0..=20 {
                        fence.spawn((
                            Mesh3d(horizontal.clone()),
                            MeshMaterial3d(wire.clone()),
                            Transform::from_xyz(0.0, j as f32 * 0.1, 0.0)
                                .with_rotation(Quat::from_rotation_z(FRAC_PI_2)),
                        ));
                    }
                });
            ctx.features
                .push("fence slides sideways, driven by frame count (deterministic)".into());
        }
    }
}

/// Camera components specific to the scene / variant (feature under test).
pub fn camera_extras(kind: SceneKind, variant: &Variant, ctx: &mut SceneCtx, camera: Entity) {
    let mut cam = ctx.commands.entity(camera);
    match kind {
        SceneKind::Ssr => {
            cam.insert(ScreenSpaceReflections::default());
            ctx.features.push("ScreenSpaceReflections (default)".into());
        }
        SceneKind::Ssao => {
            cam.insert(ScreenSpaceAmbientOcclusion::default());
            ctx.features
                .push("ScreenSpaceAmbientOcclusion (default) + NormalPrepass".into());
        }
        SceneKind::Fog => {
            let (start, end) = (2.0, 28.0);
            cam.insert(DistanceFog {
                color: Color::srgb(0.42, 0.55, 0.72),
                falloff: FogFalloff::Linear { start, end },
                ..default()
            });
            ctx.features
                .push(format!("DistanceFog linear {start}..{end} m"));
        }
        SceneKind::Skybox => {
            let image = ctx.images.add(debug_cubemap(256));
            cam.insert(bevy::light::Skybox {
                image: Some(image),
                brightness: 1_000.0,
                ..default()
            });
            ctx.features.push("Skybox (procedural labelled cubemap)".into());
        }
        SceneKind::DeferredLighting | SceneKind::ClusteredLights | SceneKind::Taa => {}
    }
    if variant.taa {
        ctx.commands
            .entity(camera)
            .insert(bevy::anti_alias::taa::TemporalAntiAliasing::default());
        ctx.features.push("TemporalAntiAliasing".into());
    }
}

/// Frame-count driven animation (only the TAA scene moves).
pub fn animate_fence(frame: u32, fences: &mut Query<&mut Transform, With<Fence>>) {
    for mut t in fences.iter_mut() {
        t.translation.x = 0.35 * (frame as f32 * 0.045).sin();
    }
}

fn repeat_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::nearest()
    })
}

/// 2x2-cell checker texture (64 px per cell), sampled with nearest + repeat.
fn checker_image(a: [u8; 3], b: [u8; 3]) -> Image {
    const CELL: u32 = 64;
    let size = CELL * 2;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let c = if (x / CELL + y / CELL).is_multiple_of(2) { a } else { b };
            data.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = repeat_sampler();
    image
}

/// Recognisable cubemap: each face has its own colour, a grid every 1/8 of the face, a
/// thick centre cross and a white border, so any zoom/offset of the sky is obvious.
/// Face order is +X, -X, +Y, -Y, +Z, -Z.
fn debug_cubemap(face: u32) -> Image {
    const COLORS: [[u8; 3]; 6] = [
        [200, 50, 50],   // +X red
        [40, 170, 190],  // -X cyan
        [70, 90, 220],   // +Y blue
        [110, 80, 50],   // -Y brown
        [60, 170, 60],   // +Z green
        [200, 170, 40],  // -Z yellow
    ];
    let cell = face / 8;
    let mut data = Vec::with_capacity((face * face * 4 * 6) as usize);
    for color in COLORS {
        for y in 0..face {
            for x in 0..face {
                let border = x < 3 || y < 3 || x >= face - 3 || y >= face - 3;
                let cross = x.abs_diff(face / 2) < 4 || y.abs_diff(face / 2) < 4;
                let grid = x % cell == 0 || y % cell == 0;
                let c = if border {
                    [255, 255, 255]
                } else if cross {
                    [20, 20, 20]
                } else if grid {
                    color.map(|v| v / 2)
                } else {
                    color
                };
                data.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: face,
            height: face,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image
}
