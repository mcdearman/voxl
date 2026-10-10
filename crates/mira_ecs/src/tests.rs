use super::prelude::*;

#[derive(Debug, PartialEq)]
struct Pos(i32);
impl Component for Pos {}

#[derive(Debug, PartialEq)]
struct Vel(i32);
impl Component for Vel {}

struct Marker;
impl Component for Marker {}

#[derive(Default)]
struct Counter(usize);

fn run<M>(world: &mut World, systems: impl super::IntoSystems<M>) {
    let mut schedule = Schedule::default();
    schedule.add_systems(systems);
    schedule.run(world);
}

/// A query guesses that an entity sits at the same place in every component's storage as in
/// the one it is walking. The guess must never matter to what the query finds.
#[test]
fn a_query_finds_the_same_whatever_order_components_were_added_in() {
    let mut world = World::new();
    let entities: Vec<Entity> = (0..40).map(|_| world.spawn(())).collect();
    // Positions in order, velocities backwards, markers on every third; then holes knocked
    // in each, so that removal's swapping has moved things too.
    for (i, &entity) in entities.iter().enumerate() {
        world.insert(entity, Pos(i as i32));
    }
    for (i, &entity) in entities.iter().enumerate().rev() {
        world.insert(entity, Vel(i as i32 * 10));
    }
    for &entity in entities.iter().step_by(3) {
        world.insert(entity, Marker);
    }
    world.remove::<Pos>(entities[5]);
    world.remove::<Vel>(entities[30]);
    world.despawn(entities[12]);
    // An entity made since, in the slot the despawned one had.
    let late = world.spawn((Vel(-10), Pos(-1), Marker));

    let mut pairs: Vec<(i32, i32)> = world
        .query::<(&Pos, &Vel)>()
        .iter()
        .map(|(pos, vel)| (pos.0, vel.0))
        .collect();
    pairs.sort_unstable();
    let mut expected: Vec<(i32, i32)> = (0..40)
        .filter(|i| ![5, 30, 12].contains(i))
        .map(|i| (i, i * 10))
        .chain([(-1, -10)])
        .collect();
    expected.sort_unstable();
    assert_eq!(pairs, expected);

    // Walking the markers, the guess is wrong for nearly everything else.
    let mut marked: Vec<i32> = world
        .query_filtered::<&Pos, (With<Marker>, With<Vel>)>()
        .iter()
        .map(|pos| pos.0)
        .collect();
    marked.sort_unstable();
    let mut expected: Vec<i32> = (0..40)
        .step_by(3)
        .filter(|i| ![12, 30].contains(i))
        .chain([-1])
        .collect();
    expected.sort_unstable();
    assert_eq!(marked, expected);
    assert_eq!(
        world
            .query_filtered::<Entity, Without<Vel>>()
            .iter()
            .count(),
        1
    );

    // Written through a query, each value lands on its own entity.
    for (mut pos, vel) in &mut world.query::<(&mut Pos, &Vel)>() {
        pos.0 = vel.0 + 1;
    }
    assert_eq!(world.get::<Pos>(entities[7]), Some(&Pos(71)));
    assert_eq!(world.get::<Pos>(late), Some(&Pos(-9)));
    assert_eq!(
        world.get::<Pos>(entities[30]),
        Some(&Pos(30)),
        "it has no Vel"
    );
    // Asked for by name, with no walk to guess from.
    assert_eq!(
        world.query::<(&Pos, &Vel)>().get(entities[39]),
        Some((&Pos(391), &Vel(390)))
    );
}

#[test]
fn stale_entities_are_rejected() {
    let mut world = World::new();
    let a = world.spawn(Pos(1));
    assert!(world.despawn(a));
    let b = world.spawn(Pos(2));
    assert_eq!(a.index(), b.index());
    assert_ne!(a, b);
    assert!(world.get::<Pos>(a).is_none());
    assert_eq!(world.get::<Pos>(b), Some(&Pos(2)));
    assert!(!world.despawn(a));
}

#[test]
fn remove_keeps_other_entities_intact() {
    let mut world = World::new();
    let entities: Vec<_> = (0..5).map(|i| world.spawn(Pos(i))).collect();
    assert_eq!(world.remove::<Pos>(entities[1]), Some(Pos(1)));
    for (i, e) in entities.iter().enumerate().filter(|(i, _)| *i != 1) {
        assert_eq!(world.get::<Pos>(*e), Some(&Pos(i as i32)));
    }
}

#[test]
fn systems_query_and_mutate() {
    let mut world = World::new();
    world.spawn((Pos(0), Vel(2)));
    world.spawn((Pos(10), Vel(-1)));
    world.spawn(Pos(100));

    fn movement(mut query: Query<(&mut Pos, &Vel)>) {
        for (mut pos, vel) in &mut query {
            pos.0 += vel.0;
        }
    }
    run(&mut world, movement);

    let mut positions: Vec<i32> = world.query::<&Pos>().iter().map(|p| p.0).collect();
    positions.sort();
    assert_eq!(positions, [2, 9, 100]);
}

#[test]
fn filters() {
    let mut world = World::new();
    world.spawn((Pos(1), Marker));
    world.spawn(Pos(2));

    let with: Vec<_> = world
        .query_filtered::<&Pos, With<Marker>>()
        .iter()
        .map(|p| p.0)
        .collect();
    assert_eq!(with, [1]);
    let without: Vec<_> = world
        .query_filtered::<&Pos, Without<Marker>>()
        .iter()
        .map(|p| p.0)
        .collect();
    assert_eq!(without, [2]);
    let optional = world.query::<(Entity, Option<&Marker>)>();
    assert_eq!(optional.iter().filter(|(_, m)| m.is_some()).count(), 1);
    assert_eq!(optional.count(), 2);
}

#[test]
fn change_detection() {
    let mut world = World::new();
    let a = world.spawn(Pos(0));
    let b = world.spawn(Pos(0));

    fn count_changed(query: Query<&Pos, Changed<Pos>>, mut out: ResMut<Counter>) {
        out.0 = query.count();
    }
    let mut schedule = Schedule::default();
    schedule.add_systems(count_changed);
    world.init_resource::<Counter>();

    schedule.run(&mut world);
    assert_eq!(
        world.resource::<Counter>().0,
        2,
        "new components count as changed"
    );

    schedule.run(&mut world);
    assert_eq!(world.resource::<Counter>().0, 0);

    world.get_mut::<Pos>(a).unwrap().0 = 5;
    schedule.run(&mut world);
    assert_eq!(world.resource::<Counter>().0, 1);

    // Reading through `Mut` without writing doesn't mark anything.
    fn touch(mut query: Query<&mut Pos>) {
        for pos in &mut query {
            let _ = pos.0;
        }
    }
    run(&mut world, touch);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Counter>().0, 0);
    let _ = b;
}

#[test]
fn commands_are_applied_after_the_system() {
    let mut world = World::new();

    fn spawner(mut commands: Commands, query: Query<&Pos>) {
        assert_eq!(query.count(), 0);
        let e = commands.spawn(Pos(7)).id();
        commands.entity(e).insert(Vel(1));
        commands.insert_resource(Counter(3));
    }
    run(&mut world, spawner);

    let query = world.query::<(&Pos, &Vel)>();
    assert_eq!(query.single().0, &Pos(7));
    assert_eq!(world.resource::<Counter>().0, 3);
}

#[test]
fn locals_persist_between_runs() {
    let mut world = World::new();
    world.init_resource::<Counter>();
    fn tick(mut calls: Local<usize>, mut out: ResMut<Counter>) {
        *calls += 1;
        out.0 = *calls;
    }
    let mut schedule = Schedule::default();
    schedule.add_systems(tick);
    for _ in 0..3 {
        schedule.run(&mut world);
    }
    assert_eq!(world.resource::<Counter>().0, 3);
}

#[test]
fn events_are_seen_once_per_reader() {
    struct Ping(u32);
    let mut world = World::new();
    world.init_resource::<Events<Ping>>();
    world.init_resource::<Counter>();

    fn send(mut writer: EventWriter<Ping>, mut n: Local<u32>) {
        *n += 1;
        writer.send(Ping(*n));
    }
    fn receive(mut reader: EventReader<Ping>, mut out: ResMut<Counter>) {
        out.0 += reader.read().map(|p| p.0 as usize).sum::<usize>();
    }
    let mut schedule = Schedule::default();
    schedule.add_systems((send, receive, super::event_update_system::<Ping>));
    schedule.run(&mut world);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Counter>().0, 1 + 2);
}

#[test]
fn disjoint_queries_are_allowed() {
    let mut world = World::new();
    world.spawn((Pos(1), Marker));
    world.spawn(Pos(2));
    fn swap(mut a: Query<&mut Pos, With<Marker>>, mut b: Query<&mut Pos, Without<Marker>>) {
        let mut a = a.single_mut();
        let mut b = b.single_mut();
        std::mem::swap(&mut a.0, &mut b.0);
    }
    run(&mut world, swap);
    let marked = world.query_filtered::<&Pos, With<Marker>>();
    assert_eq!(marked.single(), &Pos(2));
}

#[test]
fn mutable_filter_on_same_component_is_allowed() {
    let mut world = World::new();
    world.spawn(Pos(1));
    fn bump(mut q: Query<&mut Pos, Changed<Pos>>) {
        for mut p in &mut q {
            p.0 += 1;
        }
    }
    run(&mut world, bump);
    assert_eq!(world.query::<&Pos>().single(), &Pos(2));
}

#[test]
#[should_panic(expected = "conflicts with another query")]
fn conflicting_queries_panic() {
    fn bad(_a: Query<&mut Pos>, _b: Query<&Pos>) {}
    run(&mut World::new(), bad);
}

#[test]
#[should_panic(expected = "both mutably and immutably")]
fn aliasing_within_a_query_panics() {
    fn bad(_a: Query<(&mut Pos, &Pos)>) {}
    run(&mut World::new(), bad);
}

#[test]
#[should_panic(expected = "accessed mutably")]
fn conflicting_resources_panic() {
    fn bad(_a: Res<Counter>, _b: ResMut<Counter>) {}
    let mut world = World::new();
    world.init_resource::<Counter>();
    run(&mut world, bad);
}

#[test]
fn exclusive_systems() {
    let mut world = World::new();
    run(&mut world, |world: &mut World| {
        world.spawn(Pos(1));
    });
    assert_eq!(world.entity_count(), 1);
}

mod blobs {
    use std::{
        alloc::Layout,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::super::{storage::BlobSet, ComponentKey, ErasedStorage, World};

    fn read<T: Copy>(world: &World, key: ComponentKey, entity: super::Entity) -> Option<T> {
        let storage = world.erased_storage(key)?;
        // SAFETY: the test holds the world and the type matches the registered layout.
        unsafe { Some(*storage.value_ptr(entity)?.cast::<T>()) }
    }

    #[test]
    fn values_survive_growth_and_removal() {
        let mut world = World::new();
        let id = world
            .register_blob_component("test.Wide", Layout::new::<[u64; 3]>(), None)
            .unwrap();
        let key = world.named_component(id).unwrap().key;
        let entities: Vec<_> = (0..40u64).map(|_| world.spawn_empty()).collect();
        for (i, e) in entities.iter().enumerate() {
            let value = [i as u64, 7, !(i as u64)];
            // SAFETY: `value` is a valid `[u64; 3]`.
            assert!(unsafe { world.insert_raw(*e, key, value.as_ptr().cast()) });
        }
        world.remove_by_key(entities[3], key);
        world.despawn(entities[10]);
        for (i, e) in entities.iter().enumerate() {
            let expected = (i != 3 && i != 10).then_some([i as u64, 7, !(i as u64)]);
            assert_eq!(read::<[u64; 3]>(&world, key, *e), expected);
        }
        // Replacing in place.
        let value = [1u64, 2, 3];
        unsafe { world.insert_raw(entities[0], key, value.as_ptr().cast()) };
        assert_eq!(read::<[u64; 3]>(&world, key, entities[0]), Some(value));
    }

    #[test]
    fn unaligned_sources_and_empty_types() {
        let mut world = World::new();
        let id = world
            .register_blob_component("test.Aligned", Layout::new::<u128>(), None)
            .unwrap();
        let key = world.named_component(id).unwrap().key;
        let e = world.spawn_empty();
        let mut bytes = [0u8; 17];
        bytes[1..].copy_from_slice(&0x0123_4567_89ab_cdef_u128.to_ne_bytes());
        unsafe { world.insert_raw(e, key, bytes[1..].as_ptr()) };
        assert_eq!(read::<u128>(&world, key, e), Some(0x0123_4567_89ab_cdef));

        let tag = world
            .register_blob_component("test.Tag", Layout::new::<()>(), None)
            .unwrap();
        let tag = world.named_component(tag).unwrap().key;
        for _ in 0..10 {
            let e = world.spawn_empty();
            unsafe { world.insert_raw(e, tag, std::ptr::dangling()) };
            assert!(world.erased_storage(tag).unwrap().contains(e));
        }
        assert_eq!(world.erased_storage(tag).unwrap().entities().len(), 10);
    }

    static DROPS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn count_drop(_: *mut u8) {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }

    #[test]
    fn destructors_run_once_per_value() {
        let layout = Layout::new::<u32>();
        let mut world = World::new();
        let entities: Vec<_> = (0..4).map(|_| world.spawn_empty()).collect();
        let mut set = BlobSet::new(layout, Some(count_drop));
        for e in &entities {
            unsafe { set.insert_raw(*e, 5u32.to_ne_bytes().as_ptr(), 1) };
        }
        unsafe { set.insert_raw(entities[0], 6u32.to_ne_bytes().as_ptr(), 1) }; // replaces: 1
        set.remove_entity(entities[1]); // 2
        drop(set); // the remaining three: 5
        assert_eq!(DROPS.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn re_registering_keeps_values_unless_the_layout_changes() {
        let mut world = World::new();
        let id = world
            .register_blob_component("test.Kept", Layout::new::<u32>(), None)
            .unwrap();
        let key = world.named_component(id).unwrap().key;
        let e = world.spawn_empty();
        unsafe { world.insert_raw(e, key, 9u32.to_ne_bytes().as_ptr()) };

        assert_eq!(
            world.register_blob_component("test.Kept", Layout::new::<u32>(), None),
            Ok(id)
        );
        assert_eq!(read::<u32>(&world, key, e), Some(9));

        assert_eq!(
            world.register_blob_component("test.Kept", Layout::new::<u64>(), None),
            Ok(id)
        );
        assert_eq!(read::<u64>(&world, key, e), None);
    }

    #[test]
    fn exported_rust_components_are_reachable_by_name() {
        #[repr(C)]
        #[derive(Clone, Copy, PartialEq, Debug)]
        struct Speed(f32);
        impl super::Component for Speed {}

        let mut world = World::new();
        let id = world.export_component::<Speed>("test.Speed");
        assert_eq!(world.named_component_id("test.Speed"), Some(id));
        assert!(world
            .register_blob_component("test.Speed", Layout::new::<f32>(), None)
            .is_err());
        let e = world.spawn(Speed(2.0));
        let key = world.named_component(id).unwrap().key;
        assert_eq!(read::<f32>(&world, key, e), Some(2.0));
        unsafe { world.insert_raw(e, key, 3.5f32.to_ne_bytes().as_ptr()) };
        assert_eq!(world.get::<Speed>(e), Some(&Speed(3.5)));
    }
}

mod scheduling {
    use super::super::{not, resource_exists, IntoSystems};
    use super::*;

    #[derive(Default)]
    struct Order(Vec<&'static str>);

    fn a(mut order: ResMut<Order>) {
        order.0.push("a");
    }
    fn b(mut order: ResMut<Order>) {
        order.0.push("b");
    }
    fn c(mut order: ResMut<Order>) {
        order.0.push("c");
    }
    fn d(mut order: ResMut<Order>) {
        order.0.push("d");
    }

    fn order_of<M>(systems: impl IntoSystems<M>) -> Vec<&'static str> {
        let mut world = World::new();
        world.init_resource::<Order>();
        run(&mut world, systems);
        std::mem::take(&mut world.resource_mut::<Order>().0)
    }

    #[test]
    fn systems_run_in_the_order_added_unless_told_otherwise() {
        assert_eq!(order_of((a, b, c, d)), ["a", "b", "c", "d"]);
        assert_eq!(order_of((a.after(c), b, c, d)), ["b", "c", "a", "d"]);
        // A system waits for the ones it must follow; nothing is brought forward.
        assert_eq!(order_of((a, b, c, d.before(a))), ["b", "c", "d", "a"]);
        assert_eq!(order_of((a, b.after(d), c, d)), ["a", "c", "d", "b"]);
        assert_eq!(
            order_of(((d, c, b, a).chain(), a)),
            ["d", "c", "b", "a", "a"]
        );
        // Naming a system that isn't there changes nothing.
        assert_eq!(order_of((a.after(d), b.before(d))), ["a", "b"]);
    }

    #[test]
    fn sets_order_groups_of_systems() {
        let systems = (
            d.after("forces"),
            (a, b).in_set("forces"),
            c.before("forces"),
        );
        assert_eq!(order_of(systems), ["c", "a", "b", "d"]);
        // Constraints given to a group apply to each of its systems, and add up.
        let systems = (a, (b, c).before(a).after(d), d);
        assert_eq!(order_of(systems), ["d", "b", "c", "a"]);
    }

    #[test]
    #[should_panic(expected = "ordered in a circle")]
    fn a_circle_of_constraints_is_refused() {
        order_of((a.after(b), b.after(c), c.after(a), d));
    }

    #[test]
    fn the_order_survives_systems_being_added_and_replaced() {
        let mut world = World::new();
        world.init_resource::<Order>();
        let mut schedule = Schedule::default();
        schedule.add_systems((a.after(b), b));
        schedule.initialize(&mut world);
        assert_eq!(schedule.system_names().count(), 2);
        schedule.run(&mut world);
        // Added later, and still placed by its constraints.
        schedule.add_systems(c.before(b));
        schedule.run(&mut world);
        assert_eq!(world.resource::<Order>().0, ["b", "a", "c", "b", "a"]);
        let names: Vec<_> = schedule
            .system_names()
            .map(|name| name.rsplit("::").next().unwrap())
            .collect();
        assert_eq!(names, ["c", "b", "a"]);
    }

    struct Paused;

    #[test]
    fn conditions_decide_whether_a_system_runs() {
        let mut world = World::new();
        world.init_resource::<Order>();
        world.insert_resource(Counter(0));
        let mut schedule = Schedule::default();
        schedule.add_systems((
            a.run_if(not(resource_exists::<Paused>)),
            b.run_if(resource_exists::<Paused>),
            // Two conditions: both must hold. A closure with parameters is a condition too.
            (c, d)
                .run_if(|counter: Res<Counter>| counter.0 >= 2)
                .run_if(not(resource_exists::<Paused>)),
            |mut counter: ResMut<Counter>| counter.0 += 1,
        ));
        schedule.initialize(&mut world);
        let mut frame = |world: &mut World| {
            schedule.run(world);
            std::mem::take(&mut world.resource_mut::<Order>().0)
        };
        assert_eq!(frame(&mut world), ["a"]);
        assert_eq!(frame(&mut world), ["a"]);
        assert_eq!(frame(&mut world), ["a", "c", "d"]);
        world.insert_resource(Paused);
        assert_eq!(frame(&mut world), ["b"]);
        world.remove_resource::<Paused>();
        assert_eq!(frame(&mut world), ["a", "c", "d"]);
    }

    #[test]
    fn a_system_that_was_held_back_still_sees_what_changed_meanwhile() {
        let mut world = World::new();
        world.insert_resource(Counter(0));
        let entity = world.spawn(Pos(0));
        let mut schedule = Schedule::default();
        schedule.add_systems(
            (|changed: Query<&Pos, Changed<Pos>>, mut counter: ResMut<Counter>| {
                counter.0 += changed.iter().count();
            })
            .run_if(not(resource_exists::<Paused>)),
        );
        schedule.run(&mut world);
        assert_eq!(world.resource::<Counter>().0, 1);
        world.insert_resource(Paused);
        world.get_mut::<Pos>(entity).unwrap().0 = 5;
        schedule.run(&mut world);
        schedule.run(&mut world);
        assert_eq!(world.resource::<Counter>().0, 1, "held back");
        world.remove_resource::<Paused>();
        schedule.run(&mut world);
        assert_eq!(
            world.resource::<Counter>().0,
            2,
            "and the change was not missed"
        );
        schedule.run(&mut world);
        assert_eq!(world.resource::<Counter>().0, 2);
    }
}

mod batches {
    use super::super::{resource_exists, IntoSystems};
    use super::*;

    fn moves(mut q: Query<(&mut Pos, &Vel)>) {
        for (mut pos, vel) in &mut q {
            pos.0 += vel.0;
        }
    }
    fn reads_pos(_: Query<&Pos>) {}
    fn reads_vel(_: Query<&Vel>) {}
    fn writes_vel(_: Query<&mut Vel>) {}
    fn writes_marked_pos(_: Query<&mut Pos, With<Marker>>) {}
    fn writes_unmarked_pos(_: Query<&mut Pos, Without<Marker>>) {}
    fn counts(mut counter: ResMut<Counter>) {
        counter.0 += 1;
    }
    fn reads_count(_: Res<Counter>) {}
    fn whole_world(_: &mut World) {}

    fn plan<M>(systems: impl IntoSystems<M>) -> Vec<usize> {
        let mut world = World::new();
        world.insert_resource(Counter(0));
        let mut schedule = Schedule::default();
        schedule.add_systems(systems);
        schedule.initialize(&mut world);
        schedule
            .systems()
            .iter()
            .map(|s| s.batch.unwrap())
            .collect()
    }

    #[test]
    fn systems_that_touch_nothing_in_common_share_a_batch() {
        // Readers together; a writer of what they read comes after; then readers again.
        assert_eq!(
            plan((reads_pos, reads_vel, reads_count, moves, reads_pos)),
            [0, 0, 0, 1, 2]
        );
        // Writers of different things, and of the same thing on entities that can't be the
        // same, go together.
        assert_eq!(
            plan((writes_vel, writes_marked_pos, writes_unmarked_pos, counts)),
            [0, 0, 0, 0]
        );
        assert_eq!(plan((writes_vel, reads_vel)), [0, 1]);
        assert_eq!(plan((counts, reads_count, counts)), [0, 1, 2]);
        // A system that takes the whole world is alone.
        assert_eq!(
            plan((reads_pos, whole_world, reads_vel, reads_pos)),
            [0, 1, 2, 2]
        );
    }

    #[test]
    fn order_and_conditions_count() {
        // Told to run one after the other, they can't be at the same moment, clash or not.
        assert_eq!(plan((reads_pos, reads_vel).chain()), [0, 1]);
        assert_eq!(
            plan((
                reads_vel,
                reads_pos.after("early"),
                reads_vel.in_set("early")
            )),
            [0, 0, 1]
        );
        // What a condition reads is part of what its system touches.
        assert_eq!(
            plan((counts, reads_pos.run_if(resource_exists::<Counter>))),
            [0, 1]
        );
        assert_eq!(
            plan((reads_count, reads_pos.run_if(resource_exists::<Counter>))),
            [0, 0]
        );
    }
    #[test]
    fn systems_that_need_the_main_thread_are_marked() {
        struct Screen;
        fn draws(_: Res<Screen>) {}
        fn asks_first(_: Query<&Pos>) {}

        let mut world = World::new();
        world.insert_resource(Counter(0));
        world.insert_resource(Screen);
        world.pin_to_main_thread::<Screen>();
        let mut schedule = Schedule::default();
        schedule.add_systems((
            reads_pos,
            draws,
            whole_world,
            counts,
            asks_first.run_if(resource_exists::<Screen>),
        ));
        assert_eq!(
            schedule.systems()[0].main_thread,
            None,
            "not known before it is initialized"
        );
        schedule.initialize(&mut world);
        let pinned: Vec<bool> = schedule
            .systems()
            .iter()
            .map(|s| s.main_thread.unwrap())
            .collect();
        // A pinned resource, the whole world, and a condition that reads a pinned resource.
        assert_eq!(pinned, [false, true, true, false, true]);
    }
}

mod together {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        thread::ThreadId,
        time::{Duration, Instant},
    };

    use super::super::{guard, pool::Pool, IntoSystems};
    use super::*;

    /// How many systems are inside `meet` right now, and the most there have been at once.
    #[derive(Default)]
    struct Meeting {
        inside: AtomicUsize,
        most: AtomicUsize,
    }

    /// Waits a while for another system to be running at the same moment.
    fn meet(meeting: &Meeting) {
        let inside = meeting.inside.fetch_add(1, Ordering::SeqCst) + 1;
        meeting.most.fetch_max(inside, Ordering::SeqCst);
        let started = Instant::now();
        while meeting.most.load(Ordering::SeqCst) < 2 && started.elapsed() < Duration::from_secs(5)
        {
            std::thread::yield_now();
        }
        meeting.inside.fetch_sub(1, Ordering::SeqCst);
    }

    fn first(meeting: Res<Meeting>, _: Query<&Pos>) {
        meet(&meeting);
    }
    fn second(meeting: Res<Meeting>, _: Query<&mut Vel>) {
        meet(&meeting);
    }

    #[test]
    fn a_batch_really_runs_at_the_same_moment() {
        let mut world = World::new();
        world.init_resource::<Meeting>();
        let mut schedule = Schedule::default();
        schedule.add_systems((first, second));
        schedule.run(&mut world);
        let expected = if Pool::global().workers() > 0 { 2 } else { 1 };
        assert_eq!(
            world.resource::<Meeting>().most.load(Ordering::SeqCst),
            expected
        );

        // Told to take turns, they never meet. (Each then waits out its five seconds, so
        // this half only checks the plan.)
        let mut schedule = Schedule::default();
        schedule.add_systems((first, second).chain());
        schedule.initialize(&mut world);
        let batches: Vec<_> = schedule
            .systems()
            .iter()
            .map(|s| s.batch.unwrap())
            .collect();
        assert_eq!(batches, [0, 1]);
    }

    #[derive(Default)]
    struct Threads(std::sync::Mutex<Vec<(&'static str, ThreadId)>>);

    #[test]
    fn pinned_systems_stay_on_the_calling_thread() {
        struct Screen;
        fn draws(_: Res<Screen>, threads: Res<Threads>) {
            threads
                .0
                .lock()
                .unwrap()
                .push(("draws", std::thread::current().id()));
        }
        fn thinks(threads: Res<Threads>, _: Query<&Pos>) {
            threads
                .0
                .lock()
                .unwrap()
                .push(("thinks", std::thread::current().id()));
        }
        fn dreams(threads: Res<Threads>, _: Query<&Vel>) {
            threads
                .0
                .lock()
                .unwrap()
                .push(("dreams", std::thread::current().id()));
        }
        let mut world = World::new();
        world.insert_resource(Screen);
        world.pin_to_main_thread::<Screen>();
        world.init_resource::<Threads>();
        let mut schedule = Schedule::default();
        schedule.add_systems((thinks, draws, dreams));
        for _ in 0..20 {
            schedule.run(&mut world);
        }
        let here = std::thread::current().id();
        let seen = world.resource::<Threads>().0.lock().unwrap();
        assert_eq!(seen.len(), 60);
        assert!(seen
            .iter()
            .filter(|(name, _)| *name == "draws")
            .all(|(_, id)| *id == here));
        if Pool::global().workers() > 0 {
            assert!(
                seen.iter().any(|(_, id)| *id != here),
                "something ran elsewhere"
            );
        }
    }

    // A small world that exercises what could go wrong: writers of different components,
    // writers of one component on disjoint entities, readers, change detection, commands.

    fn fall(mut q: Query<(&mut Pos, &Vel)>) {
        for (mut pos, vel) in &mut q {
            pos.0 += vel.0;
        }
    }
    fn tire(mut q: Query<&mut Vel, With<Marker>>) {
        for mut vel in &mut q {
            vel.0 -= 1;
        }
    }
    fn hurry(mut q: Query<&mut Vel, Without<Marker>>) {
        for mut vel in &mut q {
            vel.0 += 2;
        }
    }
    fn tally(q: Query<&Pos, Changed<Pos>>, mut counter: ResMut<Counter>) {
        counter.0 += q.iter().count();
    }
    fn breed(mut commands: Commands, q: Query<&Pos>) {
        if q.iter().count() < 40 {
            commands.spawn((Pos(1), Vel(1)));
            commands.spawn((Pos(2), Vel(0), Marker));
        }
    }
    fn census(q: Query<Entity>, mut seen: ResMut<Seen>) {
        seen.0.push(q.iter().count());
    }
    #[derive(Default)]
    struct Seen(Vec<usize>);

    fn play(parallel: bool) -> (Vec<i32>, Vec<i32>, usize, Vec<usize>) {
        let mut world = World::new();
        world.insert_resource(Counter(0));
        world.init_resource::<Seen>();
        for i in 0..6 {
            world.spawn((Pos(i), Vel(i % 3)));
            world.spawn((Pos(-i), Vel(1), Marker));
        }
        let mut schedule = Schedule::default();
        schedule.add_systems((tire, hurry, fall, tally, breed, census, tire, hurry));
        schedule.set_parallel(parallel);
        for _ in 0..25 {
            schedule.run(&mut world);
        }
        let mut pos: Vec<i32> = world.query::<&Pos>().iter().map(|p| p.0).collect();
        let mut vel: Vec<i32> = world.query::<&Vel>().iter().map(|v| v.0).collect();
        pos.sort_unstable();
        vel.sort_unstable();
        let seen = std::mem::take(&mut world.resource_mut::<Seen>().0);
        (pos, vel, world.resource::<Counter>().0, seen)
    }

    #[test]
    fn running_together_gives_what_taking_turns_gives() {
        let turns = play(false);
        for _ in 0..5 {
            assert_eq!(play(true), turns);
        }
        // `census` comes after `breed`, and so sees what it spawned that very frame.
        assert_eq!(turns.3[0], 14);
    }

    #[test]
    fn a_failure_beside_other_systems_is_caught_alone() {
        fn fragile(_: Query<&Pos>) {
            panic!("fell over beside the others");
        }
        let mut world = World::new();
        world.insert_resource(Counter(0));
        world.init_resource::<Meeting>();
        let mut schedule = Schedule::default();
        schedule.add_systems((fragile, counts_up, reads_vel_only));
        schedule.set_guarded(true);
        schedule.run(&mut world);
        schedule.run(&mut world);
        let failures = schedule.take_failures();
        assert_eq!(failures.len(), 1, "once, then it is left out");
        assert_eq!(failures[0].message, "fell over beside the others");
        assert!(failures[0].system.ends_with("fragile"));
        if !cfg!(miri) {
            assert!(
                failures[0].stack.contains("fragile"),
                "{}",
                failures[0].stack
            );
        }
        assert_eq!(world.resource::<Counter>().0, 2, "the others carried on");
        assert!(!guard::active());

        // Unguarded, the panic comes out of `run`, after the rest of the batch has finished.
        let mut schedule = Schedule::default();
        schedule.add_systems((fragile, counts_up, reads_vel_only));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| schedule.run(&mut world)));
        assert!(result.is_err());
    }

    fn counts_up(mut counter: ResMut<Counter>) {
        counter.0 += 1;
    }
    fn reads_vel_only(_: Query<&Vel>) {}
}

mod hooks {
    use super::*;

    /// What the hooks saw, in order.
    #[derive(Default)]
    struct Heard(Vec<String>);

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<Heard>();
        world.on_add::<Pos>(|world, entity| {
            // Everything inserted with it is already there.
            let with_vel = world.has::<Vel>(entity);
            let at = world.get::<Pos>(entity).unwrap().0;
            world
                .resource_mut::<Heard>()
                .0
                .push(format!("pos {at} arrived, vel: {with_vel}"));
        });
        world.on_remove::<Pos>(|world, entity| {
            let at = world.get::<Pos>(entity).expect("still there to be read").0;
            world
                .resource_mut::<Heard>()
                .0
                .push(format!("pos {at} leaving"));
        });
        world
    }

    fn heard(world: &mut World) -> Vec<String> {
        std::mem::take(&mut world.resource_mut::<Heard>().0)
    }

    #[test]
    fn hooks_hear_components_arrive_and_leave() {
        let mut world = world();
        let a = world.spawn((Pos(1), Vel(0)));
        let b = world.spawn(Vel(5));
        assert_eq!(heard(&mut world), ["pos 1 arrived, vel: true"]);

        // Replacing a component is not an arrival; putting one where there was none is.
        world.insert(a, Pos(2));
        world.insert(b, Pos(7));
        assert_eq!(heard(&mut world), ["pos 7 arrived, vel: true"]);

        assert_eq!(world.remove::<Pos>(a), Some(Pos(2)));
        assert_eq!(world.remove::<Pos>(a), None);
        world.remove::<Vel>(b);
        assert_eq!(heard(&mut world), ["pos 2 leaving"]);
        world.despawn(b);
        world.despawn(a);
        assert_eq!(heard(&mut world), ["pos 7 leaving"]);
        assert_eq!(world.entity_count(), 0);
    }

    #[test]
    fn hooks_can_change_the_world_and_commands_reach_them() {
        let mut world = world();
        // A marker follows every `Vel`, kept by hooks rather than by a system that looks.
        world.on_add::<Vel>(|world, entity| {
            world.insert(entity, Marker);
        });
        world.on_remove::<Vel>(|world, entity| {
            world.remove::<Marker>(entity);
        });
        // Whatever has a marker when it is despawned takes a friend with it.
        world.insert_resource(Counter(0));
        world.on_remove::<Marker>(|world, _| world.resource_mut::<Counter>().0 += 1);

        let entity = world.spawn(Vel(1));
        assert!(world.has::<Marker>(entity));
        world.remove::<Vel>(entity);
        assert!(!world.has::<Marker>(entity));
        assert_eq!(world.resource::<Counter>().0, 1);

        // Through commands, hooks run when the commands are applied.
        run(&mut world, |mut commands: Commands| {
            commands.spawn((Pos(3), Vel(3)));
        });
        assert_eq!(heard(&mut world), ["pos 3 arrived, vel: true"]);
        assert_eq!(world.query::<&Marker>().iter().count(), 1);

        // A hook that despawns the entity it is told about, while it is being despawned.
        world.on_remove::<Pos>(|world, entity| {
            world.despawn(entity);
        });
        let doomed = world.spawn((Pos(9), Vel(9)));
        heard(&mut world);
        assert!(world.despawn(doomed) || !world.contains_entity(doomed));
        assert!(!world.contains_entity(doomed));
        assert_eq!(
            heard(&mut world),
            ["pos 9 leaving"],
            "told once, not once per despawn"
        );
    }
}

/// One query's work shared between threads comes to what the same work in a row would.
#[test]
fn a_querys_work_is_shared_between_threads_and_all_of_it_is_done() {
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    // Enough entities that the work is handed out; fewer under Miri, which is slow.
    let count = if cfg!(miri) { 400 } else { 20_000 };
    let mut world = World::new();
    for i in 0..count {
        let entity = world.spawn((Pos(i), Vel(2)));
        if i % 3 == 0 {
            world.insert(entity, Marker);
        }
    }
    // A few without a velocity, which the query must pass over.
    for _ in 0..10 {
        world.spawn(Pos(-1));
    }

    fn step(mut moving: Query<(&mut Pos, &Vel)>) {
        moving.par_for_each_mut(|(mut pos, vel)| pos.0 += vel.0);
    }
    run(&mut world, step);
    let moved: Vec<i32> = world.query::<(&Pos, &Vel)>().iter().map(|(pos, _)| pos.0).collect();
    assert_eq!(moved.len(), count as usize);
    assert!(moved.iter().enumerate().all(|(i, pos)| *pos == i as i32 + 2));
    assert_eq!(world.query::<&Pos>().iter().filter(|pos| pos.0 == -1).count(), 10);

    // Read only, with a filter, each match met exactly once.
    static SUM: AtomicI64 = AtomicI64::new(0);
    static MET: AtomicUsize = AtomicUsize::new(0);
    fn add_up(marked: Query<(Entity, &Pos), With<Marker>>) {
        marked.par_for_each(|(_, pos)| {
            SUM.fetch_add(pos.0 as i64, Ordering::Relaxed);
            MET.fetch_add(1, Ordering::Relaxed);
        });
    }
    run(&mut world, add_up);
    let marked = (0..count as i64).filter(|i| i % 3 == 0);
    assert_eq!(MET.load(Ordering::Relaxed), marked.clone().count());
    assert_eq!(SUM.load(Ordering::Relaxed), marked.map(|i| i + 2).sum::<i64>());

    // Too few to hand out: done where they stand, and still done.
    let mut small = World::new();
    for i in 0..5 {
        small.spawn((Pos(i), Vel(1)));
    }
    run(&mut small, step);
    assert_eq!(small.query::<&Pos>().iter().map(|pos| pos.0).sum::<i32>(), 15);
}

/// Systems that run side by side may each share out a query of their own.
#[test]
fn systems_running_side_by_side_each_share_out_their_own_query() {
    let count = if cfg!(miri) { 300 } else { 5_000 };
    let mut world = World::new();
    for i in 0..count {
        world.spawn((Pos(i), Vel(i)));
    }
    fn positions(mut all: Query<&mut Pos>) {
        all.par_for_each_mut(|mut pos| pos.0 += 1);
    }
    fn velocities(mut all: Query<&mut Vel>) {
        all.par_for_each_mut(|mut vel| vel.0 *= 2);
    }
    let mut schedule = Schedule::default();
    schedule.add_systems((positions, velocities));
    for _ in 0..3 {
        schedule.run(&mut world);
    }
    let after: Vec<(i32, i32)> = world.query::<(&Pos, &Vel)>().iter().map(|(p, v)| (p.0, v.0)).collect();
    assert!(after.iter().enumerate().all(|(i, (p, v))| *p == i as i32 + 3 && *v == i as i32 * 8));
}

/// An event aimed at an entity is heard at once by what observes it, anywhere or there.
#[test]
fn an_event_aimed_at_an_entity_is_heard_by_its_observers() {
    struct Hit(i32);
    #[derive(Default)]
    struct Heard(Vec<(Entity, &'static str, i32)>);
    let mut world = World::new();
    world.insert_resource(Heard::default());
    let (wall, door) = (world.spawn(Pos(10)), world.spawn(Pos(5)));
    world.observe::<Hit>(|world, entity, hit| {
        world.resource_mut::<Heard>().0.push((entity, "anywhere", hit.0));
        // Every hit wears the thing down; one that finishes it takes it away.
        let left = world.get_mut::<Pos>(entity).map(|pos| {
            pos.0 -= hit.0;
            pos.0
        });
        if left.is_some_and(|left| left <= 0) {
            world.despawn(entity);
        }
    });
    world.observe_entity::<Hit>(door, |world, entity, hit| {
        world.resource_mut::<Heard>().0.push((entity, "the door", hit.0));
    });

    assert_eq!(world.trigger(wall, Hit(3)), 1);
    assert_eq!(world.trigger(door, Hit(2)), 2);
    assert_eq!(world.get::<Pos>(wall), Some(&Pos(7)));
    // The blow that ends the door is not passed on to what listened on the door itself.
    assert_eq!(world.trigger(door, Hit(9)), 1);
    assert!(!world.contains_entity(door));
    assert_eq!(world.trigger(door, Hit(1)), 0);
    // Nothing listens for this kind of event.
    assert_eq!(world.trigger(wall, "a word"), 0);
    assert_eq!(
        world.resource::<Heard>().0,
        [(wall, "anywhere", 3), (door, "anywhere", 2), (door, "the door", 2), (door, "anywhere", 9)]
    );

    // From a system, through commands: heard when they are applied.
    fn strike(mut commands: Commands, walls: Query<Entity, With<Pos>>) {
        for wall in &walls {
            commands.entity(wall).trigger(Hit(1));
        }
    }
    run(&mut world, strike);
    assert_eq!(world.get::<Pos>(wall), Some(&Pos(6)));
}

/// What a stage's systems wrote is told, once for each entity, when the stage has run.
#[test]
fn a_component_that_was_written_is_told_of_when_the_stage_has_run() {
    let mut world = World::new();
    world.insert_resource(Counter(0));
    let (a, b) = (world.spawn((Pos(1), Vel(0))), world.spawn((Pos(2), Vel(0))));
    world.on_change::<Pos>(|world, entity| {
        world.resource_mut::<Counter>().0 += 1;
        // Keeps something else in step; writing the watched component here is not told again.
        let pos = world.get::<Pos>(entity).map_or(0, |pos| pos.0);
        world.insert(entity, Vel(pos * 10));
        world.get_mut::<Pos>(entity).unwrap().0 += 0;
    });
    // Nothing written since the hook was added.
    world.tell_of_changes();
    assert_eq!(world.resource::<Counter>().0, 0);

    fn move_first(mut all: Query<&mut Pos>) {
        for mut pos in &mut all {
            if pos.0 == 1 {
                // Written twice, told once.
                pos.0 = 5;
                pos.0 = 7;
            }
        }
    }
    run(&mut world, move_first);
    assert_eq!(world.resource::<Counter>().0, 1);
    assert_eq!((world.get::<Vel>(a), world.get::<Vel>(b)), (Some(&Vel(70)), Some(&Vel(0))));
    // Run again with nothing to write: nothing is told, the hook's own writing included.
    run(&mut world, move_first);
    assert_eq!(world.resource::<Counter>().0, 1);
    // An arrival counts as a writing.
    let c = world.spawn(Pos(3));
    world.tell_of_changes();
    assert_eq!(world.resource::<Counter>().0, 2);
    assert_eq!(world.get::<Vel>(c), Some(&Vel(30)));
}
