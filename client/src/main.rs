//! le-mollet native game client (Bevy). The server stays the authority (physics, trainer, routes,
//! multiplayer); this client fetches the world, renders it as realistically as it can (IGN aerial
//! photo on the real relief, physically based sky and sun) and sends the rider's commands.
//!
//!   cargo run --release -p le-mollet-client -- --course alpe-dhuez
//!   Up/Down power ±10 W (demo mode), Esc quit.

mod net;
mod world;

use anyhow::{Context, Result};
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::light_consts::lux;
use bevy::light::{AtmosphereEnvironmentMapLight, CascadeShadowConfigBuilder};
use bevy::pbr::{Atmosphere, AtmosphereSettings, ScatteringMedium, ScreenSpaceAmbientOcclusion};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::Hdr;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use clap::Parser;
use le_mollet::course::Course;
use net::{Frame, Server};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use world::{CHUNK, Terrain};

#[derive(Parser)]
#[command(about = "le-mollet game client: ride a course of a le-mollet server in 3D")]
struct Args {
    /// Server base URL (`le-mollet serve`)
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    server: String,
    /// Course id, e.g. alpe-dhuez
    #[arg(long, default_value = "alpe-dhuez")]
    course: String,
    /// Rider name shown to the others
    #[arg(long, default_value = "Cycliste")]
    name: String,
    /// Rider weight, kg (the bike adds 8)
    #[arg(long, default_value_t = 75.0)]
    weight: f32,
    /// Demo mode starting power, W
    #[arg(long, default_value_t = 200.0)]
    power: f32,
    /// Demo mode: start at this distance along the course, m
    #[arg(long)]
    seek: Option<f32>,
    /// Save a screenshot to this file after `--after` seconds, then quit (tests)
    #[arg(long)]
    screenshot: Option<String>,
    #[arg(long, default_value_t = 25.0)]
    after: f32,
}

/// Terrain chunks kept around the rider, in chunks (64 cells, 320 m).
const RADIUS: i32 = 4;

type Tile = ((usize, usize), Result<Image>);

#[derive(Resource)]
struct Game {
    server: Server,
    id: String,
    session: u64,
    course: Course,
    terrain: Terrain,
    /// Aerial photo tile size in terrain cells, when the server has photos for this course.
    tile_cells: Option<usize>,
    frames: Mutex<Receiver<Frame>>,
    last: Frame,
    /// Distance shown on screen: follows the server's, extrapolated between its 3 updates a second.
    shown: f32,
    power: f32,
    screenshot: Option<(String, f32)>,
}

#[derive(Resource)]
struct Tiles {
    tx: Sender<Tile>,
    rx: Mutex<Receiver<Tile>>,
    requested: HashSet<(usize, usize)>,
    ready: HashMap<(usize, usize), Handle<StandardMaterial>>,
    fallback: Handle<StandardMaterial>,
    trees: Handle<StandardMaterial>,
}

#[derive(Resource, Default)]
struct Chunks(HashMap<(i32, i32), Entity>);

#[derive(Component)]
struct Rider;
#[derive(Component)]
struct Hud;

fn main() -> Result<()> {
    let a = Args::parse();
    let server = Server::new(&a.server);
    eprintln!("loading {} from {}...", a.course, a.server);
    let course: Course = server.json(&format!("/course?id={}", a.course))?;
    let terrain = Terrain::parse(&server.bytes(&format!("/terrain?id={}", a.course)).context("this course has no terrain")?, &course.points[0])?;
    let tile_cells = server.json::<serde_json::Value>(&format!("/ortho?id={}&f=meta.json", a.course)).ok().and_then(|m| m["tile_cells"].as_u64()).map(|n| n as usize);
    if tile_cells.is_none() {
        eprintln!("no aerial photo for this course: run `le-mollet ortho courses/{}.json` on the server", a.course);
    }
    let session = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos() as u64 % (1 << 52);
    let frames = server.ride(&a.course, session, &a.name, a.weight + 8.0)?;
    server.send(format!("/power?session={session}&w={}", a.power));
    if let Some(d) = a.seek {
        server.send(format!("/seek?session={session}&d={d}"));
    }
    let game = Game {
        server,
        id: a.course,
        session,
        course,
        terrain,
        tile_cells,
        frames: Mutex::new(frames),
        last: Frame::default(),
        shown: a.seek.unwrap_or(0.0),
        power: a.power,
        screenshot: a.screenshot.map(|p| (p, a.after)),
    };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window { title: "Le Mollet".into(), resolution: (1400u32, 900u32).into(), ..default() }),
            ..default()
        }))
        .insert_resource(game)
        .insert_resource(GlobalAmbientLight::NONE)
        .init_resource::<Chunks>()
        .add_systems(Startup, setup)
        .add_systems(Update, (ride, stream_tiles, stream_chunks, controls, hud, screenshot))
        .run();
    Ok(())
}

fn setup(
    mut commands: Commands,
    game: Res<Game>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut media: ResMut<Assets<ScatteringMedium>>,
) {
    // Camera: physically based sky and aerial perspective, sun-sized exposure, filmic tonemapping.
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 60f32.to_radians(), far: 40_000.0, near: 0.2, ..default() }),
        Hdr,
        Atmosphere::earthlike(media.add(ScatteringMedium::default())),
        AtmosphereSettings::default(),
        AtmosphereEnvironmentMapLight::default(),
        Exposure { ev100: 13.5 },
        Tonemapping::AcesFitted,
        Bloom::NATURAL,
        Msaa::Off,
        TemporalAntiAliasing::default(),
        ScreenSpaceAmbientOcclusion::default(),
        Transform::from_xyz(0.0, 800.0, 10.0),
    ));
    // Sun, high in the south-west, shadows in cascades out to 800 m.
    commands.spawn((
        DirectionalLight { shadows_enabled: true, illuminance: lux::RAW_SUNLIGHT, ..default() },
        Transform::from_xyz(-0.45, 0.75, 0.5).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder { num_cascades: 4, first_cascade_far_bound: 25.0, maximum_distance: 800.0, ..default() }.build(),
    ));

    let t = &game.terrain;
    let photo = |path: &str, srgb: bool, repeat: bool, images: &mut Assets<Image>| {
        game.server.bytes(path).and_then(|b| world::decode(&b, srgb, repeat)).map(|i| images.add(i)).map_err(|e| eprintln!("{path}: {e:#}")).ok()
    };
    // Far mountains under the overview photo.
    let far = materials.add(StandardMaterial {
        base_color_texture: game.tile_cells.and_then(|_| photo(&format!("/ortho?id={}&f=overview.jpg", game.id), true, false, &mut images)),
        base_color: if game.tile_cells.is_some() { Color::srgb(0.85, 0.85, 0.85) } else { Color::srgb(0.35, 0.45, 0.3) },
        perceptual_roughness: 1.0,
        reflectance: 0.2,
        ..default()
    });
    commands.spawn((Mesh3d(meshes.add(world::far_mesh(t))), MeshMaterial3d(far)));

    // Road: CC0 asphalt from the server's texture set, white markings.
    let (mut road, marks) = world::road_meshes(t, &game.course.points);
    let _ = road.generate_tangents();
    let asphalt = materials.add(StandardMaterial {
        base_color_texture: photo("/textures/road_color.jpg", true, true, &mut images),
        normal_map_texture: photo("/textures/road_normal.jpg", false, true, &mut images),
        base_color: Color::srgb(0.55, 0.55, 0.55),
        perceptual_roughness: 0.85,
        ..default()
    });
    commands.spawn((Mesh3d(meshes.add(road)), MeshMaterial3d(asphalt)));
    commands.spawn((Mesh3d(meshes.add(marks)), MeshMaterial3d(materials.add(StandardMaterial { base_color: Color::srgb(0.92, 0.92, 0.9), perceptual_roughness: 0.6, ..default() }))));

    let (tx, rx) = channel();
    commands.insert_resource(Tiles {
        tx,
        rx: Mutex::new(rx),
        requested: HashSet::new(),
        ready: HashMap::new(),
        fallback: materials.add(StandardMaterial { base_color: Color::srgb(0.35, 0.42, 0.3), perceptual_roughness: 1.0, ..default() }),
        trees: materials.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.95, ..default() }),
    });

    spawn_rider(&mut commands, &mut meshes, &mut materials);

    commands.spawn((
        Hud,
        Text::new(""),
        TextFont { font_size: 20.0, ..default() },
        TextColor(Color::WHITE),
        Node { position_type: PositionType::Absolute, top: px(14), left: px(14), padding: UiRect::all(px(12)), ..default() },
        BackgroundColor(Color::srgba(0.05, 0.07, 0.09, 0.65)),
    ));
}

/// A road cyclist from primitives, facing -Z (Bevy's forward): wheels, frame, rider on the hoods.
fn spawn_rider(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) {
    let mat = |m: &mut Assets<StandardMaterial>, c: Color, r: f32| m.add(StandardMaterial { base_color: c, perceptual_roughness: r, ..default() });
    let (tire, frame, jersey, shorts, skin, helmet) = (
        mat(materials, Color::srgb(0.05, 0.05, 0.05), 0.8),
        mat(materials, Color::srgb(0.75, 0.08, 0.05), 0.35),
        mat(materials, Color::srgb(0.8, 0.1, 0.08), 0.7),
        mat(materials, Color::srgb(0.04, 0.04, 0.05), 0.6),
        mat(materials, Color::srgb(0.85, 0.65, 0.5), 0.6),
        mat(materials, Color::srgb(0.95, 0.95, 0.95), 0.3),
    );
    let rod = meshes.add(Cylinder::new(1.0, 1.0));
    let wheel = meshes.add(Torus::new(0.32, 0.35));
    let ball = meshes.add(Sphere::new(1.0));
    let segment = |a: Vec3, b: Vec3, r: f32| {
        let d = b - a;
        Transform::from_translation((a + b) / 2.0).with_rotation(Quat::from_rotation_arc(Vec3::Y, d.normalize())).with_scale(Vec3::new(r, d.length(), r))
    };
    commands
        .spawn((Rider, Transform::default(), Visibility::default()))
        .with_children(|p| {
            for z in [-0.52, 0.52] {
                p.spawn((Mesh3d(wheel.clone()), MeshMaterial3d(tire.clone()), Transform::from_xyz(0.0, 0.34, z).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))));
            }
            let (bb, seat, head, rear, front) = (Vec3::new(0.0, 0.28, 0.08), Vec3::new(0.0, 0.85, 0.22), Vec3::new(0.0, 0.8, -0.4), Vec3::new(0.0, 0.34, 0.52), Vec3::new(0.0, 0.34, -0.52));
            for (a, b) in [(bb, seat), (seat, head), (bb, head - Vec3::Y * 0.12), (bb, rear), (seat, rear), (head, front)] {
                p.spawn((Mesh3d(rod.clone()), MeshMaterial3d(frame.clone()), segment(a, b, 0.022)));
            }
            let (hip, shoulder) = (Vec3::new(0.0, 1.0, 0.24), Vec3::new(0.0, 1.32, -0.28));
            p.spawn((Mesh3d(rod.clone()), MeshMaterial3d(jersey.clone()), segment(hip, shoulder, 0.17)));
            p.spawn((Mesh3d(ball.clone()), MeshMaterial3d(skin.clone()), Transform::from_xyz(0.0, 1.47, -0.4).with_scale(Vec3::splat(0.11))));
            p.spawn((Mesh3d(ball.clone()), MeshMaterial3d(helmet.clone()), Transform::from_xyz(0.0, 1.52, -0.38).with_scale(Vec3::new(0.13, 0.09, 0.16))));
            for x in [-0.12, 0.12] {
                let (knee, foot) = (Vec3::new(x, 0.62, -0.08), Vec3::new(x * 1.2, 0.2, 0.1));
                p.spawn((Mesh3d(rod.clone()), MeshMaterial3d(shorts.clone()), segment(hip + Vec3::X * x, knee, 0.075)));
                p.spawn((Mesh3d(rod.clone()), MeshMaterial3d(skin.clone()), segment(knee, foot, 0.055)));
                p.spawn((Mesh3d(rod.clone()), MeshMaterial3d(skin.clone()), segment(shoulder + Vec3::X * x * 1.5, Vec3::new(x * 1.6, 0.95, -0.5), 0.045)));
            }
        });
}

/// Latest server frame, smoothed distance, rider and chase camera.
fn ride(time: Res<Time>, mut game: ResMut<Game>, mut rider: Single<&mut Transform, With<Rider>>, mut cam: Single<&mut Transform, (With<Camera3d>, Without<Rider>)>) {
    let dt = time.delta_secs().min(0.1);
    let latest = game.frames.lock().unwrap().try_iter().last();
    if let Some(f) = latest {
        game.last = f;
    }
    let (reported, speed) = (game.last.distance, game.last.speed_kmh);
    game.shown += speed / 3.6 * dt;
    game.shown += (reported - game.shown) * (dt * 2.0).min(1.0);
    let (p, dir) = world::point_at(&game.terrain, &game.course.points, game.shown);
    rider.translation = p;
    rider.look_to(dir, Vec3::Y);
    let want = p - dir * 7.0 + Vec3::Y * 2.6;
    let k = 1.0 - (-dt * 4.0).exp();
    cam.translation = if cam.translation.distance(want) > 200.0 { want } else { cam.translation.lerp(want, k) };
    cam.look_at(p + dir * 12.0 + Vec3::Y * 0.8, Vec3::Y);
}

/// Aerial photo tiles: decoded with mips on worker threads, turned into materials here.
fn stream_tiles(mut tiles: ResMut<Tiles>, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let done: Vec<Tile> = tiles.rx.lock().unwrap().try_iter().collect();
    for (key, img) in done {
        let mat = match img {
            Ok(img) => materials.add(StandardMaterial {
                base_color_texture: Some(images.add(img)),
                base_color: Color::srgb(0.72, 0.72, 0.72), // the photo already holds its light: keep it from washing out under the sun
                perceptual_roughness: 1.0,
                reflectance: 0.2,
                ..default()
            }),
            Err(e) => {
                eprintln!("aerial photo tile {key:?}: {e:#}");
                tiles.fallback.clone()
            }
        };
        tiles.ready.insert(key, mat);
    }
}

/// Smooth photo terrain around the rider: request tiles, build one missing chunk per frame, drop far ones.
fn stream_chunks(
    mut commands: Commands,
    game: Res<Game>,
    mut tiles: ResMut<Tiles>,
    mut chunks: ResMut<Chunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    rider: Single<&Transform, With<Rider>>,
) {
    let t = &game.terrain;
    let (nx, ny) = t.chunks();
    let here = (((rider.translation.x - t.ox) / t.cx / CHUNK as f32).floor() as i32, ((rider.translation.z - t.oz) / t.cz / CHUNK as f32).floor() as i32);
    let mut wanted: Vec<(i32, i32)> = (-RADIUS..=RADIUS)
        .flat_map(|j| (-RADIUS..=RADIUS).map(move |i| (here.0 + i, here.1 + j)))
        .filter(|&(i, j)| i >= 0 && j >= 0 && (i as usize) < nx && (j as usize) < ny && !chunks.0.contains_key(&(i, j)))
        .collect();
    wanted.sort_by_key(|&(i, j)| (i - here.0).pow(2) + (j - here.1).pow(2));
    for (i, j) in wanted {
        let (mesh, tile) = world::chunk_mesh(t, i as usize, j as usize, game.tile_cells.unwrap_or(usize::MAX / 2));
        let material = match game.tile_cells {
            None => tiles.fallback.clone(),
            Some(_) => match tiles.ready.get(&tile) {
                Some(m) => m.clone(),
                None => {
                    if tiles.requested.insert(tile) {
                        let (server, tx, id) = (game.server.clone(), tiles.tx.clone(), game.id.clone());
                        std::thread::spawn(move || {
                            let img = server.bytes(&format!("/ortho?id={id}&f={}_{}.jpg", tile.0, tile.1)).and_then(|b| world::decode(&b, true, false));
                            let _ = tx.send((tile, img));
                        });
                    }
                    continue; // build it once its photo is here
                }
            },
        };
        let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::default()));
        if let Some(trees) = world::tree_mesh(t, i as usize, j as usize) {
            e.with_child((Mesh3d(meshes.add(trees)), MeshMaterial3d(tiles.trees.clone())));
        }
        chunks.0.insert((i, j), e.id());
        break; // one per frame keeps frame times flat
    }
    chunks.0.retain(|&(i, j), e| {
        let keep = (i - here.0).abs().max((j - here.1).abs()) <= RADIUS + 1;
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
}

fn controls(keys: Res<ButtonInput<KeyCode>>, mut game: ResMut<Game>, mut exit: MessageWriter<AppExit>) {
    let dw = if keys.just_pressed(KeyCode::ArrowUp) { 10.0 } else if keys.just_pressed(KeyCode::ArrowDown) { -10.0 } else { 0.0 };
    if dw != 0.0 {
        game.power = (game.power + dw).clamp(0.0, 2000.0);
        game.server.send(format!("/power?session={}&w={}", game.session, game.power));
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

fn hud(game: Res<Game>, mut text: Single<&mut Text, With<Hud>>) {
    let f = &game.last;
    let s = f.elapsed as u32;
    **text = Text::new(format!(
        "{}\n{:>4.0} W   {:>4.1} km/h   {:>4.1} %\n{:.2} / {:.1} km   {:.0} m   {}:{:02}:{:02}\nHaut/Bas : puissance ({:.0} W)   Esc : quitter",
        game.course.name,
        f.power,
        f.speed_kmh,
        f.grade,
        f.distance / 1000.0,
        game.course.length() / 1000.0,
        f.ele,
        s / 3600,
        s / 60 % 60,
        s % 60,
        game.power,
    ));
}

/// Test helper: `--screenshot file --after s` saves one frame, then quits.
fn screenshot(mut commands: Commands, time: Res<Time>, mut game: ResMut<Game>, mut exit: MessageWriter<AppExit>, mut taken: Local<Option<f32>>) {
    let Some((path, after)) = game.screenshot.clone() else { return };
    let now = time.elapsed_secs();
    match *taken {
        None if now > after => {
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
            *taken = Some(now);
        }
        Some(at) if now > at + 2.0 => {
            game.screenshot = None;
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
