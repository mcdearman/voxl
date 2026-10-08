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
