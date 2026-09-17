use std::{collections::HashSet, hash::Hash};

use glam::Vec2;
pub use winit::{event::MouseButton, keyboard::KeyCode};

use crate::{
    app::{App, Plugin, Stage},
    ecs::ResMut,
};

/// Pressed state for keys or mouse buttons. `just_*` sets are cleared at the end of each frame.
pub struct ButtonInput<T: Copy + Eq + Hash> {
    pressed: HashSet<T>,
    just_pressed: HashSet<T>,
    just_released: HashSet<T>,
}

impl<T: Copy + Eq + Hash> Default for ButtonInput<T> {
    fn default() -> Self {
        Self {
            pressed: HashSet::new(),
            just_pressed: HashSet::new(),
            just_released: HashSet::new(),
        }
    }
}

impl<T: Copy + Eq + Hash> ButtonInput<T> {
    pub fn press(&mut self, input: T) {
        if self.pressed.insert(input) {
            self.just_pressed.insert(input);
        }
    }

    pub fn release(&mut self, input: T) {
        if self.pressed.remove(&input) {
            self.just_released.insert(input);
        }
    }

    pub fn release_all(&mut self) {
        self.just_released.extend(self.pressed.drain());
    }

    pub fn pressed(&self, input: T) -> bool {
        self.pressed.contains(&input)
    }

    pub fn just_pressed(&self, input: T) -> bool {
        self.just_pressed.contains(&input)
    }

    pub fn just_released(&self, input: T) -> bool {
        self.just_released.contains(&input)
    }

    pub fn any_pressed(&self, inputs: impl IntoIterator<Item = T>) -> bool {
        inputs.into_iter().any(|i| self.pressed(i))
    }

    pub fn get_pressed(&self) -> impl Iterator<Item = &T> {
        self.pressed.iter()
    }

    pub fn clear(&mut self) {
        self.just_pressed.clear();
        self.just_released.clear();
    }
}

/// Mouse motion and scroll accumulated over the current frame.
#[derive(Default, Debug)]
pub struct Mouse {
    /// Raw device motion. Unaffected by cursor grabbing or window edges, so use this for mouselook.
    pub delta: Vec2,
    pub scroll: Vec2,
    /// Cursor position in physical pixels, if the cursor is over the window.
    pub position: Option<Vec2>,
}

fn clear_input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut mouse: ResMut<Mouse>,
) {
    keys.clear();
    buttons.clear();
    mouse.delta = Vec2::ZERO;
    mouse.scroll = Vec2::ZERO;
}

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Mouse>()
            .add_systems(Stage::Last, clear_input);
    }
}
