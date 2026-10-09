//! Relations between entities: a component that names another entity, and the way back.
//!
//! A relation is an ordinary component with an entity in it, which may carry more:
//!
//! ```ignore
//! struct OwnedBy(Entity);
//! impl Component for OwnedBy {}
//! impl Relation for OwnedBy {
//!     fn target(&self) -> Entity {
//!         self.0
//!     }
//! }
//!
//! app.add_relation::<OwnedBy>();
//! ```
//!
//! From then on every entity that something is `OwnedBy` has a [`Related<OwnedBy>`]: the
//! entities that are, kept by the engine. So a rule can be read in either direction with
//! plain queries: `Query<&OwnedBy>` for whose a thing is, `Query<&Related<OwnedBy>>` for
//! what a player has. [`Parent`](crate::transform::Parent) is one of these, and `Children`
//! is its `Related`.

use std::{collections::HashMap, fmt, marker::PhantomData};

use crate::{
    app::{App, Stage},
    ecs::{Component, Entity, World},
};

/// A component that relates its entity to another one.
pub trait Relation: Component {
    /// The entity this one is related to.
    fn target(&self) -> Entity;
}

/// The entities whose `R` names this one, in entity order. The engine keeps it up to date
/// from the `R` components every frame; read it, and change the `R`s to change it.
pub struct Related<R: Relation>(Vec<Entity>, PhantomData<fn() -> R>);

impl<R: Relation> Component for Related<R> {}

impl<R: Relation> Related<R> {
    fn new(entities: Vec<Entity>) -> Self {
        Self(entities, PhantomData)
    }

    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.0.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.0.binary_search(&entity).is_ok()
    }

    pub fn as_slice(&self) -> &[Entity] {
        &self.0
    }
}

impl<R: Relation> Clone for Related<R> {
    fn clone(&self) -> Self {
        Self::new(self.0.clone())
    }
}

impl<R: Relation> Default for Related<R> {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl<R: Relation> PartialEq for Related<R> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<R: Relation> Eq for Related<R> {}

impl<R: Relation> fmt::Debug for Related<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Related").field(&self.0).finish()
    }
}

/// What a relation's links looked like when its `Related` was last rebuilt.
struct Links<R> {
    sources: usize,
    entities: usize,
    newest_change: u64,
    dangling: usize,
    relation: PhantomData<fn() -> R>,
}

impl<R> PartialEq for Links<R> {
    fn eq(&self, other: &Self) -> bool {
        (
            self.sources,
            self.entities,
            self.newest_change,
            self.dangling,
        ) == (
            other.sources,
            other.entities,
            other.newest_change,
            other.dangling,
        )
    }
}

/// Every source of `R`, under the entity it names.
fn by_target<R: Relation>(world: &mut World) -> HashMap<Entity, Vec<Entity>> {
    let mut targets: HashMap<Entity, Vec<Entity>> = HashMap::new();
    for (source, relation) in world.query::<(Entity, &R)>().iter() {
        targets.entry(relation.target()).or_default().push(source);
    }
    targets
}

/// Rebuilds `Related<R>` from the `R` components when the links have changed. Looking costs
/// one pass over the `R`s; rebuilding only happens when something is different.
/// [`App::add_relation`] runs this every frame.
pub fn sync<R: Relation>(world: &mut World) {
    let mut links = Links::<R> {
        sources: 0,
        entities: world.entity_count(),
        newest_change: 0,
        dangling: 0,
        relation: PhantomData,
    };
    if let Some(relations) = world.storage::<R>() {
        links.sources = relations.len();
        for &source in relations.entities() {
            let ticks = relations.ticks(source).expect("listed, so present");
            links.newest_change = links.newest_change.max(ticks.changed);
            let target = relations.get(source).expect("listed, so present").target();
            links.dangling += !world.contains_entity(target) as usize;
        }
    }
    if world.get_resource::<Links<R>>() == Some(&links) {
        return;
    }
    world.insert_resource(links);

    let targets = by_target::<R>(world);
    // Take `Related` away from entities that nothing names any more.
    let stale: Vec<Entity> = world
        .query::<(Entity, &Related<R>)>()
        .iter()
        .filter(|(entity, _)| !targets.contains_key(entity))
        .map(|(entity, _)| entity)
        .collect();
    for entity in stale {
        world.remove::<Related<R>>(entity);
    }
    for (target, mut sources) in targets {
        if !world.contains_entity(target) {
            continue;
        }
        sources.sort_unstable();
        // Only write when the list differs, so `Changed<Related<R>>` means what it says.
        if world
            .get::<Related<R>>(target)
            .is_none_or(|old| old.0 != sources)
        {
            world.insert(target, Related::<R>::new(sources));
        }
    }
}

/// Despawns an entity, everything whose `R` names it, everything whose `R` names those, and
/// so on down. Returns how many entities were despawned.
pub fn despawn_with_related<R: Relation>(world: &mut World, entity: Entity) -> usize {
    // From the `R` components themselves, not `Related`, so that links made earlier in the
    // same frame count too.
    let mut targets = by_target::<R>(world);
    let mut pending = vec![entity];
    let mut despawned = 0;
    while let Some(next) = pending.pop() {
        if let Some(sources) = targets.remove(&next) {
            pending.extend(sources);
        }
        despawned += world.despawn(next) as usize;
    }
    despawned
}

/// The entities whose `R` names `target`, as of the last [`sync`].
pub fn related<R: Relation>(world: &World, target: Entity) -> &[Entity] {
    world
        .get::<Related<R>>(target)
        .map_or(&[], Related::as_slice)
}

/// The chain of entities reached by following `R` from `entity`: what it names, what that
/// names, and so on, nearest first. Stops at an entity that is gone, or on coming round to
/// one already passed.
pub fn ancestors<R: Relation>(world: &World, entity: Entity) -> Vec<Entity> {
    let mut chain = Vec::new();
    let mut at = entity;
    while let Some(next) = world.get::<R>(at).map(Relation::target) {
        if next == entity || chain.contains(&next) || !world.contains_entity(next) {
            break;
        }
        chain.push(next);
        at = next;
    }
    chain
}

impl App {
    /// Keeps [`Related<R>`] up to date for a relation, in `PostUpdate`.
    pub fn add_relation<R: Relation>(&mut self) -> &mut Self {
        self.add_systems(Stage::PostUpdate, sync::<R>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{Changed, Query, ResMut};

    /// A debt: a relation that carries something besides whom it is to.
    struct Owes {
        to: Entity,
        coins: u32,
    }

    impl Component for Owes {}

    impl Relation for Owes {
        fn target(&self) -> Entity {
            self.to
        }
    }

    struct Follows(Entity);

    impl Component for Follows {}

    impl Relation for Follows {
        fn target(&self) -> Entity {
            self.0
        }
    }

    fn owed(app: &App, lender: Entity) -> Vec<Entity> {
        related::<Owes>(&app.world, lender).to_vec()
    }

    #[test]
    fn the_way_back_is_kept_for_any_relation() {
        let mut app = App::new();
        app.add_relation::<Owes>().add_relation::<Follows>();
        let bank = app.world.spawn(());
        let miser = app.world.spawn(());
        let a = app.world.spawn(Owes { to: bank, coins: 5 });
        let b = app.world.spawn(Owes { to: bank, coins: 7 });
        let c = app.world.spawn((
            Owes {
                to: miser,
                coins: 1,
            },
            Follows(a),
        ));
        app.update();
        assert_eq!(owed(&app, bank), [a, b]);
        assert_eq!(owed(&app, miser), [c]);
        assert_eq!(related::<Follows>(&app.world, a), [c]);
        assert!(app.world.get::<Related<Follows>>(bank).is_none());
        assert!(app.world.get::<Related<Owes>>(bank).unwrap().contains(b));

        // What a relation carries is read with it, in either direction.
        let total: u32 = owed(&app, bank)
            .iter()
            .map(|&debtor| app.world.get::<Owes>(debtor).unwrap().coins)
            .sum();
        assert_eq!(total, 12);

        // A debt moved, a debt paid, a debtor gone: the lists follow.
        app.world.get_mut::<Owes>(b).unwrap().to = miser;
        app.update();
        assert_eq!(owed(&app, bank), [a]);
        assert_eq!(owed(&app, miser), [b, c]);
        app.world.remove::<Owes>(a);
        app.update();
        assert!(app.world.get::<Related<Owes>>(bank).is_none());
        app.world.despawn(c);
        app.update();
        assert_eq!(owed(&app, miser), [b]);
        assert!(app.world.get::<Related<Follows>>(a).is_none());

        // The lender gone, the debt names nobody; nothing is listed, and nothing breaks.
        app.world.despawn(miser);
        app.update();
        assert_eq!(app.world.get::<Owes>(b).unwrap().to, miser);
        assert!(owed(&app, miser).is_empty());
    }

    #[test]
    fn the_list_only_changes_when_the_links_do() {
        #[derive(Default)]
        struct Changes(u32);

        let mut app = App::new();
        app.add_relation::<Follows>()
            .init_resource::<Changes>()
            .add_systems(
                Stage::Last,
                |changed: Query<Entity, Changed<Related<Follows>>>, mut count: ResMut<Changes>| {
                    count.0 += changed.iter().count() as u32;
                },
            );
        let leader = app.world.spawn(());
        let first = app.world.spawn(Follows(leader));
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(app.world.resource::<Changes>().0, 1);
        app.world.spawn(Follows(leader));
        app.update();
        app.update();
        assert_eq!(app.world.resource::<Changes>().0, 2);
        // Pointing at the same entity again is no change to the list.
        app.world.insert(first, Follows(leader));
        app.update();
        assert_eq!(app.world.resource::<Changes>().0, 2);
    }

    #[test]
    fn a_chain_is_followed_and_taken_down_whole() {
        let mut app = App::new();
        app.add_relation::<Follows>();
        let head = app.world.spawn(());
        let second = app.world.spawn(Follows(head));
        let third = app.world.spawn(Follows(second));
        let other = app.world.spawn(Follows(head));
        let apart = app.world.spawn(());
        assert_eq!(ancestors::<Follows>(&app.world, third), [second, head]);
        assert!(ancestors::<Follows>(&app.world, head).is_empty());

        // Round in a circle, the walk stops where it started.
        app.world.insert(head, Follows(third));
        assert_eq!(ancestors::<Follows>(&app.world, third), [second, head]);
        app.world.remove::<Follows>(head);

        // Links made this frame count, though `Related` has not been rebuilt.
        assert_eq!(despawn_with_related::<Follows>(&mut app.world, second), 2);
        assert!(!app.world.contains_entity(third));
        assert!(app.world.contains_entity(other) && app.world.contains_entity(apart));
        assert_eq!(despawn_with_related::<Follows>(&mut app.world, head), 2);
        assert_eq!(app.world.entity_count(), 1);
    }
}
