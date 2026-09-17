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
