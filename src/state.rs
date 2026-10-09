//! States: which part of the game is running (a menu, a level, a pause screen), with systems
//! that run only in one of them and systems that run on the way in and out.
//!
//! ```ignore
//! #[derive(Clone, Copy, PartialEq, Debug)]
//! enum Game { Menu, Playing, Paused }
//!
//! app.init_state(Game::Menu)
//!     .on_enter(Game::Playing, spawn_level)
//!     .on_exit(Game::Playing, despawn_level)
//!     .add_systems(Stage::Update, move_player.run_if(in_state(Game::Playing)));
//!
//! fn start(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<Game>>) {
//!     if keys.just_pressed(KeyCode::Enter) {
//!         next.set(Game::Playing);
//!     }
//! }
//! ```

use crate::{
    app::{App, Stage},
    ecs::{IntoSystems, Res, Schedule, World},
};

/// A type whose values are the states: usually an enum.
pub trait States: Clone + PartialEq + 'static {}

impl<S: Clone + PartialEq + 'static> States for S {}

/// The state the game is in. A resource; read it, and change it through [`NextState`].
#[derive(Clone, Debug, PartialEq)]
pub struct State<S>(S);

impl<S> State<S> {
    pub fn get(&self) -> &S {
        &self.0
    }
}

/// The state to change to. The change happens early in the next frame (in
/// `Stage::PreUpdate`), running the old state's exit systems and the new one's enter systems.
#[derive(Clone, Debug)]
pub struct NextState<S>(Option<S>);

impl<S> Default for NextState<S> {
    fn default() -> Self {
        Self(None)
    }
}

impl<S> NextState<S> {
    /// Asks for a change of state. Asking for the state the game is already in leaves it
    /// and re-enters it.
    pub fn set(&mut self, state: S) {
        self.0 = Some(state);
    }
}

/// The systems to run on entering and leaving each state.
struct Transitions<S> {
    enter: Vec<(S, Schedule)>,
    exit: Vec<(S, Schedule)>,
    /// Whether the first state has been entered yet.
    begun: bool,
}

impl<S: States> Transitions<S> {
    fn schedule<'a>(list: &'a mut Vec<(S, Schedule)>, state: &S) -> &'a mut Schedule {
        let index = match list.iter().position(|(s, _)| s == state) {
            Some(index) => index,
            None => {
                list.push((state.clone(), Schedule::default()));
                list.len() - 1
            }
        };
        &mut list[index].1
    }

    fn run(list: &mut [(S, Schedule)], state: &S, world: &mut World) {
        if let Some((_, schedule)) = list.iter_mut().find(|(s, _)| s == state) {
            schedule.run(world);
        }
    }
}

/// Changes state when asked to, and enters the first state on the first frame.
fn apply_state_transitions<S: States>(world: &mut World) {
    world.resource_scope(|world, transitions: &mut Transitions<S>| {
        if !transitions.begun {
            transitions.begun = true;
            let first = world.resource::<State<S>>().0.clone();
            Transitions::run(&mut transitions.enter, &first, world);
        }
        // An enter system may ask for another change at once; a few are followed in one
        // frame, and a loop of them carries on next frame instead of hanging this one.
        for _ in 0..8 {
            let Some(next) = world.resource_mut::<NextState<S>>().0.take() else {
                break;
            };
            // The exit systems see the state being left.
            let previous = world.resource::<State<S>>().0.clone();
            Transitions::run(&mut transitions.exit, &previous, world);
            world.resource_mut::<State<S>>().0 = next.clone();
            Transitions::run(&mut transitions.enter, &next, world);
        }
    });
}

/// A run condition: holds while the game is in this state.
pub fn in_state<S: States>(state: S) -> impl Fn(Option<Res<State<S>>>) -> bool + Clone {
    move |current: Option<Res<State<S>>>| current.is_some_and(|current| current.0 == state)
}

impl App {
    /// Starts the game in a state. Its enter systems run on the first frame.
    pub fn init_state<S: States>(&mut self, initial: S) -> &mut Self {
        if self.world.contains_resource::<State<S>>() {
            return self;
        }
        self.world.insert_resource(State(initial));
        self.world.insert_resource(NextState::<S>::default());
        self.world.insert_resource(Transitions::<S> {
            enter: Vec::new(),
            exit: Vec::new(),
            begun: false,
        });
        self.add_systems(Stage::PreUpdate, apply_state_transitions::<S>)
    }

    /// Adds systems that run once each time the game enters a state.
    ///
    /// # Panics
    /// If `init_state` hasn't been called for this type of state.
    pub fn on_enter<S: States, M>(&mut self, state: S, systems: impl IntoSystems<M>) -> &mut Self {
        let transitions = self.transitions::<S>();
        Transitions::schedule(&mut transitions.enter, &state).add_systems(systems);
        self
    }

    /// Adds systems that run once each time the game leaves a state.
    ///
    /// # Panics
    /// If `init_state` hasn't been called for this type of state.
    pub fn on_exit<S: States, M>(&mut self, state: S, systems: impl IntoSystems<M>) -> &mut Self {
        let transitions = self.transitions::<S>();
        Transitions::schedule(&mut transitions.exit, &state).add_systems(systems);
        self
    }

    fn transitions<S: States>(&mut self) -> &mut Transitions<S> {
        self.world
            .get_resource_mut::<Transitions<S>>()
            .expect("call `init_state` before adding systems for a state's enter or exit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{Commands, ResMut};

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Game {
        Menu,
        Playing,
        Over,
    }

    #[derive(Default)]
    struct Log(Vec<&'static str>);

    fn note(what: &'static str) -> impl FnMut(ResMut<Log>) {
        move |mut log: ResMut<Log>| log.0.push(what)
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Log>()
            .init_state(Game::Menu)
            .on_enter(Game::Menu, note("enter menu"))
            .on_exit(Game::Menu, note("exit menu"))
            .on_enter(Game::Playing, note("enter playing"))
            .on_exit(Game::Playing, note("exit playing"))
            .add_systems(
                Stage::Update,
                (
                    note("menu").run_if(in_state(Game::Menu)),
                    note("playing").run_if(in_state(Game::Playing)),
                ),
            );
        app
    }

    fn take(app: &mut App) -> Vec<&'static str> {
        std::mem::take(&mut app.world.resource_mut::<Log>().0)
    }

    #[test]
    fn states_gate_systems_and_run_enter_and_exit_systems() {
        let mut app = app();
        app.update();
        assert_eq!(take(&mut app), ["enter menu", "menu"]);
        app.update();
        assert_eq!(take(&mut app), ["menu"]);

        app.world
            .resource_mut::<NextState<Game>>()
            .set(Game::Playing);
        assert_eq!(
            app.world.resource::<State<Game>>().get(),
            &Game::Menu,
            "not until next frame"
        );
        app.update();
        assert_eq!(take(&mut app), ["exit menu", "enter playing", "playing"]);

        // The same state again: left and entered.
        app.world
            .resource_mut::<NextState<Game>>()
            .set(Game::Playing);
        app.update();
        assert_eq!(take(&mut app), ["exit playing", "enter playing", "playing"]);

        // A state nothing was registered for.
        app.world.resource_mut::<NextState<Game>>().set(Game::Over);
        app.update();
        assert_eq!(take(&mut app), ["exit playing"]);
        assert_eq!(app.world.resource::<State<Game>>().get(), &Game::Over);
    }

    #[test]
    fn enter_systems_can_spawn_and_move_straight_on() {
        #[derive(Default)]
        struct Seen(Vec<Game>);
        let mut app = App::new();
        app.init_resource::<Seen>()
            .init_state(Game::Menu)
            // A splash state that goes straight to the game, and whose exit sees itself.
            .on_enter(
                Game::Menu,
                |mut commands: Commands, mut next: ResMut<NextState<Game>>| {
                    commands.spawn(crate::transform::Transform::IDENTITY);
                    next.set(Game::Playing);
                },
            )
            .on_exit(
                Game::Menu,
                |state: Res<State<Game>>, mut seen: ResMut<Seen>| {
                    seen.0.push(*state.get());
                },
            )
            .on_enter(
                Game::Playing,
                |state: Res<State<Game>>, mut seen: ResMut<Seen>| {
                    seen.0.push(*state.get());
                },
            );
        app.update();
        assert_eq!(app.world.resource::<Seen>().0, [Game::Menu, Game::Playing]);
        assert_eq!(app.world.entity_count(), 1);
    }
}
