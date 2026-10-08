use super::{
    system::{BoxedSystem, IntoSystem},
    world::World,
};

/// Who added a system. Systems from a native plugin carry its id so they can be swapped or
/// removed when the plugin reloads; everything else is `ENGINE`.
pub type SystemOwner = u32;
pub const ENGINE: SystemOwner = 0;

/// An ordered list of systems. Systems run one after another in the order they were added,
/// and each system's commands are applied before the next one starts.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<(SystemOwner, BoxedSystem)>,
}

impl Schedule {
    pub fn add_systems<M>(&mut self, systems: impl IntoSystems<M>) {
        self.systems
            .extend(systems.into_systems().into_iter().map(|s| (ENGINE, s)));
    }

    /// Makes `systems` the complete set belonging to `owner`. A new system takes the place of
    /// the owner's old system of the same name, so the order systems run in survives a
    /// reload; other new systems go at the end, and old ones without a successor are removed.
    pub fn replace_owned(&mut self, owner: SystemOwner, systems: Vec<BoxedSystem>) {
        let mut kept = vec![false; self.systems.len()];
        let mut added = Vec::new();
        for system in systems {
            let slot =
                self.systems.iter().enumerate().position(|(i, (o, old))| {
                    *o == owner && !kept[i] && old.name() == system.name()
                });
            match slot {
                Some(i) => {
                    self.systems[i].1 = system;
                    kept[i] = true;
                }
                None => added.push((owner, system)),
            }
        }
        let mut index = 0;
        self.systems.retain(|(o, _)| {
            let keep = *o != owner || kept[index];
            index += 1;
            keep
        });
        self.systems.extend(added);
    }

    pub fn initialize(&mut self, world: &mut World) {
        for (_, system) in &mut self.systems {
            system.initialize(world);
        }
    }

    pub fn run(&mut self, world: &mut World) {
        for (_, system) in &mut self.systems {
            system.run(world);
        }
    }

    pub fn system_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.systems.iter().map(|(_, s)| s.name())
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
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8),
    (S9, M9)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8),
    (S9, M9),
    (S10, M10)
);
impl_into_systems_tuple!(
    (S0, M0),
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8),
    (S9, M9),
    (S10, M10),
    (S11, M11)
);
