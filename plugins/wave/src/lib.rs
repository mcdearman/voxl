//! An example voxl plugin in Rust: makes everything with a `demo.Cell` ride a travelling wave.
//!
//! Run `cargo run --example plugins`, then change a number below and run
//! `cargo build -p wave` in another terminal. The running app picks the new code up, and each
//! entity keeps its `wave.Rider` (its own random phase) across the reload.

use std::sync::OnceLock;

use voxl_plugin::{App, Component, Error, Plain, Stage, System, Transform};

const HEIGHT: f32 = 1.0;
const SPEED: f32 = 2.0;
const WAVELENGTH: f32 = 6.0;

/// Defined by the host example: where the entity sits on the grid.
#[repr(C)]
#[derive(Clone, Copy)]
struct Cell {
    x: f32,
    z: f32,
}

/// Defined here: per-entity data the host knows nothing about.
#[repr(C)]
#[derive(Clone, Copy)]
struct Rider {
    phase: f32,
}

/// Plugin globals are lost on reload, so anything that must persist lives in engine-owned
/// state. Here: the component handles (re-fetched each load) and a count of reloads.
#[repr(C)]
#[derive(Clone, Copy)]
struct State {
    loads: u32,
    seed: u32,
}
unsafe impl Plain for State {}

/// What the systems need from `load`. A reload is a fresh copy of this library, so this is
/// set once per load.
struct Handles {
    rider: Component<Rider>,
    state: *mut State,
}
// SAFETY: the engine only ever calls a plugin from one thread.
unsafe impl Send for Handles {}
unsafe impl Sync for Handles {}
static HANDLES: OnceLock<Handles> = OnceLock::new();

fn load(app: &mut App) -> Result<(), Error> {
    let transform = app.lookup::<Transform>("voxl.Transform")?;
    let cell = app.lookup::<Cell>("demo.Cell")?;
    let rider = app.register::<Rider>("wave.Rider")?;

    let state = app.state::<State>("wave.state");
    // SAFETY: the engine returns a valid block, and plugin code runs on one thread.
    let loads = unsafe {
        (*state).loads += 1;
        (*state).loads
    };
    voxl_plugin::info!("wave loaded ({loads} time(s) this run)");
    let _ = HANDLES.set(Handles { rider, state });

    app.add_system(
        "adopt",
        Stage::Update,
        &[cell.with(), rider.without()],
        adopt,
    )?;
    app.add_system(
        "ride",
        Stage::Update,
        &[transform.write(), cell.read(), rider.read()],
        ride,
    )
}

fn handles() -> &'static Handles {
    HANDLES.get().expect("systems only run after `load`")
}

/// Gives every cell that doesn't have one yet a `Rider` with a random phase.
fn adopt(system: &mut System) {
    let handles = handles();
    while let Some((entity, [])) = system.next() {
        // SAFETY: the state block is valid for as long as the plugin is loaded.
        let seed = unsafe { &mut (*handles.state).seed };
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let phase = (*seed >> 8) as f32 / (1 << 24) as f32 * 0.6;
        system.insert(entity, handles.rider, &Rider { phase });
    }
}

fn ride(system: &mut System) {
    let time = system.elapsed() as f32;
    while let Some((_entity, [transform, cell, rider])) = system.next() {
        // SAFETY: the pointers are to the components the terms name, in the order given.
        let (transform, cell, rider) = unsafe {
            (
                &mut *transform.cast::<Transform>(),
                &*cell.cast::<Cell>(),
                &*rider.cast::<Rider>(),
            )
        };
        let along = (cell.x + cell.z) / WAVELENGTH * std::f32::consts::TAU;
        transform.translation[1] = HEIGHT * (along - time * SPEED + rider.phase).sin();
    }
}

voxl_plugin::export_plugin!(load);
