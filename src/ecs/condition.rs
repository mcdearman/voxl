//! Run conditions: functions with system parameters that say whether a system should run.

use std::{any::type_name, marker::PhantomData};

use super::{
    access::Access,
    query::SystemTicks,
    resource::Res,
    storage::Tick,
    system::{SystemMeta, SystemParam, SystemParamItem},
    world::World,
};

/// Something a system can be made to wait on. Made from a function by `run_if`.
pub trait Condition: 'static {
    fn initialize(&mut self, world: &mut World);
    fn check(&mut self, world: &mut World) -> bool;
    /// What the condition reads, once initialized; `None` if it can't say.
    fn access(&self) -> Option<&Access> {
        None
    }
}

pub type BoxedCondition = Box<dyn Condition>;

pub trait IntoCondition<Marker> {
    fn into_condition(self) -> BoxedCondition;
}

/// Implemented for functions that return `bool` and whose arguments are all `SystemParam`s.
pub trait ConditionFunction<Marker>: 'static {
    type Param: SystemParam;

    fn run(&mut self, param: SystemParamItem<'_, '_, Self::Param>) -> bool;
}

macro_rules! impl_condition_function {
    ($($P:ident),*) => {
        #[allow(non_snake_case)]
        impl<Func, $($P: SystemParam),*> ConditionFunction<fn($($P,)*) -> bool> for Func
        where
            Func: 'static,
            for<'a> &'a mut Func: FnMut($($P),*) -> bool + FnMut($(SystemParamItem<$P>),*) -> bool,
        {
            type Param = ($($P,)*);

            fn run(&mut self, param: SystemParamItem<'_, '_, ($($P,)*)>) -> bool {
                // As for systems: the helper makes the compiler pick the impl that takes
                // the fetched items.
                fn call_inner<$($P),*>(mut f: impl FnMut($($P),*) -> bool, $($P: $P),*) -> bool {
                    f($($P),*)
                }
                let ($($P,)*) = param;
                call_inner(self, $($P),*)
            }
        }
    };
}

impl_condition_function!();
impl_condition_function!(P0);
impl_condition_function!(P0, P1);
impl_condition_function!(P0, P1, P2);
impl_condition_function!(P0, P1, P2, P3);

struct FunctionCondition<Marker: 'static, F: ConditionFunction<Marker>> {
    func: F,
    state: Option<<F::Param as SystemParam>::State>,
    last_run: Tick,
    name: &'static str,
    access: Option<Access>,
    _marker: PhantomData<fn() -> Marker>,
}

impl<Marker: 'static, F: ConditionFunction<Marker>> Condition for FunctionCondition<Marker, F> {
    fn initialize(&mut self, world: &mut World) {
        if self.state.is_none() {
            let mut access = Access::new(self.name);
            self.state = Some(F::Param::init_state(world, &mut access));
            self.access = Some(access);
        }
    }

    fn access(&self) -> Option<&Access> {
        self.access.as_ref()
    }

    fn check(&mut self, world: &mut World) -> bool {
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
        let result = self.func.run(params);
        F::Param::apply(state, world);
        self.last_run = meta.ticks.this_run;
        result
    }
}

impl<Marker: 'static, F: ConditionFunction<Marker>> IntoCondition<Marker> for F {
    fn into_condition(self) -> BoxedCondition {
        Box::new(FunctionCondition {
            func: self,
            state: None,
            last_run: 0,
            name: type_name::<F>(),
            access: None,
            _marker: PhantomData,
        })
    }
}

/// The opposite of a condition: `spawn_wave.run_if(not(resource_exists::<Paused>))`.
pub fn not<M>(condition: impl IntoCondition<M> + Clone) -> impl IntoCondition<NotMarker> + Clone {
    Not(move || condition.clone().into_condition())
}

#[doc(hidden)]
pub struct NotMarker;

#[derive(Clone)]
struct Not<F>(F);

struct NotCondition(BoxedCondition);

impl Condition for NotCondition {
    fn initialize(&mut self, world: &mut World) {
        self.0.initialize(world);
    }

    fn check(&mut self, world: &mut World) -> bool {
        !self.0.check(world)
    }

    fn access(&self) -> Option<&Access> {
        self.0.access()
    }
}

impl<F: Fn() -> BoxedCondition> IntoCondition<NotMarker> for Not<F> {
    fn into_condition(self) -> BoxedCondition {
        Box::new(NotCondition((self.0)()))
    }
}

/// Holds while the world has a resource of this type:
/// `tick_timer.run_if(resource_exists::<RoundTimer>)`.
pub fn resource_exists<R: 'static>(resource: Option<Res<R>>) -> bool {
    resource.is_some()
}
