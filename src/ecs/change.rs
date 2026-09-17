use std::ops::{Deref, DerefMut};

use super::storage::{ComponentTicks, Tick};

/// Mutable access to a component that records a change only when it is actually written to.
pub struct Mut<'a, T: ?Sized> {
    pub(crate) value: &'a mut T,
    pub(crate) ticks: &'a mut ComponentTicks,
    pub(crate) last_run: Tick,
    pub(crate) this_run: Tick,
}

impl<'a, T: ?Sized> Mut<'a, T> {
    pub fn is_added(&self) -> bool {
        self.ticks.is_added(self.last_run)
    }

    pub fn is_changed(&self) -> bool {
        self.ticks.is_changed(self.last_run)
    }

    pub fn set_changed(&mut self) {
        self.ticks.changed = self.this_run;
    }

    /// Mutates without marking the value as changed.
    pub fn bypass_change_detection(&mut self) -> &mut T {
        self.value
    }

    pub fn reborrow(&mut self) -> Mut<'_, T> {
        Mut {
            value: self.value,
            ticks: self.ticks,
            last_run: self.last_run,
            this_run: self.this_run,
        }
    }

    pub fn into_inner(self) -> &'a mut T {
        self.ticks.changed = self.this_run;
        self.value
    }
}

impl<T: ?Sized> Deref for Mut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: ?Sized> DerefMut for Mut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.set_changed();
        self.value
    }
}
