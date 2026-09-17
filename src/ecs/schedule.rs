use super::{
    system::{BoxedSystem, IntoSystem},
    world::World,
};

/// An ordered list of systems. Systems run one after another in the order they were added,
/// and each system's commands are applied before the next one starts.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<BoxedSystem>,
}

impl Schedule {
    pub fn add_systems<M>(&mut self, systems: impl IntoSystems<M>) {
        self.systems.extend(systems.into_systems());
    }

    pub fn initialize(&mut self, world: &mut World) {
        for system in &mut self.systems {
            system.initialize(world);
        }
    }

    pub fn run(&mut self, world: &mut World) {
        for system in &mut self.systems {
            system.run(world);
        }
    }

    pub fn system_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.systems.iter().map(|s| s.name())
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}

/// A single system or a tuple of them, e.g. `app.add_systems(Stage::Update, (move_player, spin))`.
pub trait IntoSystems<Marker> {
    fn into_systems(self) -> Vec<BoxedSystem>;
}

#[doc(hidden)]
pub struct SingleSystem;

impl<M, S: IntoSystem<M>> IntoSystems<(SingleSystem, M)> for S {
    fn into_systems(self) -> Vec<BoxedSystem> {
        vec![Box::new(self.into_system())]
    }
}

#[doc(hidden)]
pub struct SystemTuple;

macro_rules! impl_into_systems_tuple {
    ($(($S:ident, $M:ident)),*) => {
        #[allow(non_snake_case)]
        impl<$($M, $S: IntoSystems<$M>),*> IntoSystems<(SystemTuple, $($M,)*)> for ($($S,)*) {
            fn into_systems(self) -> Vec<BoxedSystem> {
                let ($($S,)*) = self;
                let mut systems = Vec::new();
                $(systems.extend($S.into_systems());)*
                systems
            }
        }
    };
}

impl_into_systems_tuple!((S0, M0));
impl_into_systems_tuple!((S0, M0), (S1, M1));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2), (S3, M3));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2), (S3, M3), (S4, M4));
impl_into_systems_tuple!((S0, M0), (S1, M1), (S2, M2), (S3, M3), (S4, M4), (S5, M5));
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7)
);
