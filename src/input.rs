use std::{collections::HashSet, hash::Hash};

use glam::Vec2;
pub use winit::{event::MouseButton, keyboard::KeyCode};

use crate::{
    app::{App, Plugin, Stage},
    ecs::{ResMut, World},
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

    /// Takes a press away as if it had never happened: for when something in front of the
    /// game (a panel of its interface) has acted on it, and the game shouldn't as well.
    pub fn consume(&mut self, input: T) {
        self.pressed.remove(&input);
        self.just_pressed.remove(&input);
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

/// Something played from outside the window: by a test, a debugger, or an agent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Played {
    Key(KeyCode, bool),
    Button(MouseButton, bool),
    /// Raw mouse motion, as mouselook reads it.
    Motion(Vec2),
    /// Where the cursor is, in physical pixels.
    Cursor(Vec2),
}

/// Input waiting to be played into the game. It arrives at the start of the next frame the
/// simulation runs, so what is played into a paused game is there, pressed that very frame,
/// when the game is stepped. A resource.
#[derive(Default)]
pub struct InjectedInput {
    now: Vec<Played>,
    /// What to play after this many more frames: the releases of taps, mostly.
    later: Vec<(u32, Played)>,
    /// Text waiting to be typed.
    typed: Vec<String>,
}

impl InjectedInput {
    /// Plays something at the start of the next frame.
    pub fn play(&mut self, played: Played) {
        self.now.push(played);
    }

    /// Plays something after the next `frames` frames have run.
    pub fn play_after(&mut self, frames: u32, played: Played) {
        self.later.push((frames, played));
    }

    /// Types text, as an interface hears typing: it goes to whatever field has the keyboard
    /// and not to the game's keys, and it arrives even while the game is paused. A line
    /// break in it is Enter.
    pub fn type_text(&mut self, text: impl Into<String>) {
        self.typed.push(text.into());
    }

    /// Presses a key and lets it go `frames` frames later.
    pub fn tap(&mut self, key: KeyCode, frames: u32) {
        self.play(Played::Key(key, true));
        self.play_after(frames.max(1), Played::Key(key, false));
    }

    /// What is due this frame; the rest moves a frame closer.
    fn due(&mut self) -> Vec<Played> {
        let mut due = std::mem::take(&mut self.now);
        self.later.retain_mut(|(frames, played)| {
            if *frames == 0 {
                due.push(*played);
                return false;
            }
            *frames -= 1;
            true
        });
        due
    }
}

/// Plays injected input into the input resources at the start of a frame. In a frame the
/// simulation runs (`running`), everything that is due; in a frame it is held still, only
/// the pointer, as a real mouse would still move and click over a paused game's interface.
/// Keys wait for the game to run, so that a key pressed into a paused game is pressed on the
/// first frame stepped.
pub(crate) fn play_injected(world: &mut World, running: bool) {
    let Some(injected) = world.get_resource_mut::<InjectedInput>() else {
        return;
    };
    let typed = std::mem::take(&mut injected.typed);
    let due = if running {
        injected.due()
    } else {
        let (pointer, keys) = std::mem::take(&mut injected.now)
            .into_iter()
            .partition(|played| !matches!(played, Played::Key(..)));
        injected.now = keys;
        pointer
    };
    // An interface hears the window's own events, so input played from outside is put among
    // them as well: a click an agent makes lands on a panel as a person's would.
    let mut heard = Vec::new();
    for played in due {
        match played {
            Played::Key(key, true) => world.resource_mut::<ButtonInput<KeyCode>>().press(key),
            Played::Key(key, false) => world.resource_mut::<ButtonInput<KeyCode>>().release(key),
            Played::Button(button, down) => {
                let buttons = world.resource_mut::<ButtonInput<MouseButton>>();
                if down {
                    buttons.press(button);
                } else {
                    buttons.release(button);
                }
                heard.push(winit::event::WindowEvent::MouseInput {
                    device_id: winit::event::DeviceId::dummy(),
                    state: if down {
                        winit::event::ElementState::Pressed
                    } else {
                        winit::event::ElementState::Released
                    },
                    button,
                });
            }
            Played::Motion(delta) => world.resource_mut::<Mouse>().delta += delta,
            Played::Cursor(position) => {
                world.resource_mut::<Mouse>().position = Some(position);
                heard.push(winit::event::WindowEvent::CursorMoved {
                    device_id: winit::event::DeviceId::dummy(),
                    position: winit::dpi::PhysicalPosition::new(
                        position.x as f64,
                        position.y as f64,
                    ),
                });
            }
        }
    }
    // Typing reaches an interface the way an input method's does: as finished text.
    heard.extend(
        typed
            .into_iter()
            .map(|text| winit::event::WindowEvent::Ime(winit::event::Ime::Commit(text))),
    );
    if let Some(events) = world.get_resource_mut::<crate::window::WindowEvents>() {
        events.0.extend(heard);
    }
}

/// The key with this name, as winit names keys: `KeyW`, `Space`, `ArrowLeft`, `Digit1`,
/// `ShiftLeft`, `F5`, …
pub fn key_named(name: &str) -> Option<KeyCode> {
    use KeyCode::*;
    const KEYS: &[KeyCode] = &[
        KeyA,
        KeyB,
        KeyC,
        KeyD,
        KeyE,
        KeyF,
        KeyG,
        KeyH,
        KeyI,
        KeyJ,
        KeyK,
        KeyL,
        KeyM,
        KeyN,
        KeyO,
        KeyP,
        KeyQ,
        KeyR,
        KeyS,
        KeyT,
        KeyU,
        KeyV,
        KeyW,
        KeyX,
        KeyY,
        KeyZ,
        Digit0,
        Digit1,
        Digit2,
        Digit3,
        Digit4,
        Digit5,
        Digit6,
        Digit7,
        Digit8,
        Digit9,
        ArrowUp,
        ArrowDown,
        ArrowLeft,
        ArrowRight,
        Space,
        Enter,
        Escape,
        Tab,
        Backspace,
        Delete,
        ShiftLeft,
        ShiftRight,
        ControlLeft,
        ControlRight,
        AltLeft,
        AltRight,
        SuperLeft,
        SuperRight,
        Minus,
        Equal,
        Comma,
        Period,
        Slash,
        Semicolon,
        Quote,
        Backquote,
        BracketLeft,
        BracketRight,
        Backslash,
        Home,
        End,
        PageUp,
        PageDown,
        Insert,
        F1,
        F2,
        F3,
        F4,
        F5,
        F6,
        F7,
        F8,
        F9,
        F10,
        F11,
        F12,
    ];
    KEYS.iter()
        .copied()
        .find(|key| format!("{key:?}").eq_ignore_ascii_case(name))
}

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Mouse>()
            .init_resource::<InjectedInput>()
            .add_systems(Stage::Last, clear_input);
    }
}
