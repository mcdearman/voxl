use std::{any::type_name, marker::PhantomData};

use super::{access::Access, query::SystemTicks, storage::Tick, world::World};

/// Information a system parameter receives when it is fetched.
pub struct SystemMeta {
    pub name: &'static str,
    pub ticks: SystemTicks,
}

/// Anything that can appear as an argument of a system function.
pub trait SystemParam: Sized {
    /// Per-system state that persists between runs (e.g. the `Local` value or the command queue).
    type State: 'static;
    type Item<'w, 's>;

    /// Registers what the parameter accesses. Called once, before the first run.
    fn init_state(world: &mut World, access: &mut Access) -> Self::State;

    /// # Safety
    /// The caller must have exclusive access to the world, and the access declared in
    /// `init_state` must have been validated.
    unsafe fn fetch<'w, 's>(
        state: &'s mut Self::State,
        world: &'w World,
        meta: &SystemMeta,
    ) -> Self::Item<'w, 's>;

    /// Applies deferred work (like commands) after the system has run.
    fn apply(_state: &mut Self::State, _world: &mut World) {}
}

pub type SystemParamItem<'w, 's, P> = <P as SystemParam>::Item<'w, 's>;

macro_rules! impl_system_param_tuple {
    ($($P:ident),*) => {
        #[allow(non_snake_case, unused_variables, clippy::unused_unit)]
        impl<$($P: SystemParam),*> SystemParam for ($($P,)*) {
            type State = ($($P::State,)*);
            type Item<'w, 's> = ($($P::Item<'w, 's>,)*);

            fn init_state(world: &mut World, access: &mut Access) -> Self::State {
                ($($P::init_state(world, access),)*)
            }

            unsafe fn fetch<'w, 's>(
                state: &'s mut Self::State,
                world: &'w World,
                meta: &SystemMeta,
            ) -> Self::Item<'w, 's> {
                let ($($P,)*) = state;
                ($($P::fetch($P, world, meta),)*)
            }

            fn apply(state: &mut Self::State, world: &mut World) {
                let ($($P,)*) = state;
                $($P::apply($P, world);)*
            }
        }
    };
}

impl_system_param_tuple!();
impl_system_param_tuple!(P0);
impl_system_param_tuple!(P0, P1);
impl_system_param_tuple!(P0, P1, P2);
impl_system_param_tuple!(P0, P1, P2, P3);
impl_system_param_tuple!(P0, P1, P2, P3, P4);
impl_system_param_tuple!(P0, P1, P2, P3, P4, P5);
impl_system_param_tuple!(P0, P1, P2, P3, P4, P5, P6);
impl_system_param_tuple!(P0, P1, P2, P3, P4, P5, P6, P7);
impl_system_param_tuple!(P0, P1, P2, P3, P4, P5, P6, P7, P8);
impl_system_param_tuple!(P0, P1, P2, P3, P4, P5, P6, P7, P8, P9);
impl_system_param_tuple!(P0, P1, P2, P3, P4, P5, P6, P7, P8, P9, P10);
impl_system_param_tuple!(P0, P1, P2, P3, P4, P5, P6, P7, P8, P9, P10, P11);

pub trait System: 'static {
    fn name(&self) -> &'static str;
    fn initialize(&mut self, world: &mut World);
    fn run(&mut self, world: &mut World);
}

pub type BoxedSystem = Box<dyn System>;

pub trait IntoSystem<Marker>: Sized {
    type System: System;

    fn into_system(self) -> Self::System;
}

/// Implemented for functions whose arguments are all `SystemParam`s. `Marker` is the function's
/// signature, which keeps the impls for different arities from overlapping.
pub trait SystemParamFunction<Marker>: 'static {
    type Param: SystemParam;

    fn run(&mut self, param: SystemParamItem<'_, '_, Self::Param>);
}

macro_rules! impl_system_function {
    ($($P:ident),*) => {
        #[allow(non_snake_case)]
        impl<Func, $($P: SystemParam),*> SystemParamFunction<fn($($P,)*)> for Func
        where
            Func: 'static,
            for<'a> &'a mut Func: FnMut($($P),*) + FnMut($(SystemParamItem<$P>),*),
        {
            type Param = ($($P,)*);

            fn run(&mut self, param: SystemParamItem<'_, '_, ($($P,)*)>) {
                // Calling through a generic helper forces the compiler to pick the
                // `FnMut(Item<'w, 's>...)` impl rather than the `FnMut(P...)` one.
                #[allow(clippy::too_many_arguments)]
                fn call_inner<$($P),*>(mut f: impl FnMut($($P),*), $($P: $P),*) {
                    f($($P),*)
                }
                let ($($P,)*) = param;
                call_inner(self, $($P),*)
            }
        }
    };
}

impl_system_function!();
impl_system_function!(P0);
impl_system_function!(P0, P1);
impl_system_function!(P0, P1, P2);
impl_system_function!(P0, P1, P2, P3);
impl_system_function!(P0, P1, P2, P3, P4);
impl_system_function!(P0, P1, P2, P3, P4, P5);
impl_system_function!(P0, P1, P2, P3, P4, P5, P6);
impl_system_function!(P0, P1, P2, P3, P4, P5, P6, P7);
impl_system_function!(P0, P1, P2, P3, P4, P5, P6, P7, P8);
impl_system_function!(P0, P1, P2, P3, P4, P5, P6, P7, P8, P9);
impl_system_function!(P0, P1, P2, P3, P4, P5, P6, P7, P8, P9, P10);
impl_system_function!(P0, P1, P2, P3, P4, P5, P6, P7, P8, P9, P10, P11);

pub struct FunctionSystem<Marker: 'static, F: SystemParamFunction<Marker>> {
    func: F,
    state: Option<<F::Param as SystemParam>::State>,
    last_run: Tick,
    name: &'static str,
    _marker: PhantomData<fn() -> Marker>,
}

impl<Marker: 'static, F: SystemParamFunction<Marker>> System for FunctionSystem<Marker, F> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn initialize(&mut self, world: &mut World) {
        if self.state.is_none() {
            let mut access = Access::new(self.name);
            self.state = Some(F::Param::init_state(world, &mut access));
        }
    }

    fn run(&mut self, world: &mut World) {
        self.initialize(world);
        let meta = SystemMeta {
            name: self.name,
            ticks: SystemTicks {
                last_run: self.last_run,
                this_run: world.increment_change_tick(),
            },
        };
        let state = self.state.as_mut().unwrap();
        // SAFETY: we hold `&mut World`, and the parameters' access was validated in `initialize`.
        let params = unsafe { F::Param::fetch(state, world, &meta) };
        self.func.run(params);
        F::Param::apply(state, world);
        self.last_run = meta.ticks.this_run;
    }
}

#[doc(hidden)]
pub struct FunctionMarker;

impl<Marker: 'static, F: SystemParamFunction<Marker>> IntoSystem<(FunctionMarker, Marker)> for F {
    type System = FunctionSystem<Marker, F>;

    fn into_system(self) -> Self::System {
        FunctionSystem {
            func: self,
            state: None,
            last_run: 0,
            name: type_name::<F>(),
            _marker: PhantomData,
        }
    }
}

/// A system that takes `&mut World` directly. Useful for setup work and anything structural.
pub struct ExclusiveSystem<F> {
    func: F,
    name: &'static str,
}

impl<F: FnMut(&mut World) + 'static> System for ExclusiveSystem<F> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn initialize(&mut self, _world: &mut World) {}

    fn run(&mut self, world: &mut World) {
        world.increment_change_tick();
        (self.func)(world);
    }
}

#[doc(hidden)]
pub struct ExclusiveMarker;

impl<F: FnMut(&mut World) + 'static> IntoSystem<ExclusiveMarker> for F {
    type System = ExclusiveSystem<F>;

    fn into_system(self) -> Self::System {
        ExclusiveSystem {
            func: self,
            name: type_name::<F>(),
        }
    }
}
