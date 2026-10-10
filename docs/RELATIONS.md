# Relations

A relation says that one entity has something to do with another: this turret is mounted on
that tank, this arrow was shot by that archer, this unit belongs to that squad. In mira a
relation is an ordinary component with an entity in it, and the engine keeps the way back.

```rust
struct ShotBy(Entity);
impl Component for ShotBy {}
impl Relation for ShotBy {
    fn target(&self) -> Entity {
        self.0
    }
}

app.add_relation::<ShotBy>();
```

From then on, every entity that something was `ShotBy` has a `Related<ShotBy>`: the entities
that were, in entity order. Both directions are plain components, so both are read with
plain queries, checked and run in parallel like any other:

```rust
// From the arrow to the archer.
fn credit(arrows: Query<(&ShotBy, &Hit)>, mut archers: Query<&mut Score>) { … }

// From the archer to their arrows.
fn recall(archers: Query<(&Recalling, &Related<ShotBy>)>, mut commands: Commands) {
    for (_, arrows) in &archers {
        for arrow in arrows.iter() {
            commands.despawn(arrow);
        }
    }
}
```

A relation can carry more than whom it names (`struct Owes { to: Entity, coins: u32 }`); what
it carries is on the source, with the name.

## The hierarchy is one

`Parent` is a relation and `Children` is `Related<Parent>`. Nothing about them is special
except that transforms follow them. `despawn_recursive` is `despawn_with_related::<Parent>`.

## What the engine does

- `Related<R>` is rebuilt in `PostUpdate` when the links have changed: an `R` added,
  removed, pointed somewhere else, or an entity gone. Looking costs one pass over the `R`
  components; a frame in which nothing changed writes nothing, so `Changed<Related<R>>`
  means the list really changed.
- Until then, within the frame, `Related` is as it was. Code that must see links made this
  frame reads the `R` components (as `despawn_with_related` does), or calls
  `relation::sync::<R>(world)` itself.
- An `R` that names an entity which is gone stays as it is and is listed nowhere.

Also in `mira::relation`:

| | |
|---|---|
| `related::<R>(world, target)` | the entities whose `R` names `target` |
| `ancestors::<R>(world, entity)` | what it names, what that names, and so on, nearest first |
| `despawn_with_related::<R>(world, entity)` | the entity and everything that leads to it |

## When what is named goes

A relation says what becomes of an entity when the one it names is despawned:

```rust
impl Relation for InSquad {
    fn target(&self) -> Entity {
        self.0
    }
    const WHEN_TARGET_GOES: WhenTargetGoes = WhenTargetGoes::Despawn;
}
```

`Keep` (the default) leaves it naming an entity that is no more, for the game to see and
settle; `Unlink` takes the component off it; `Despawn` despawns it, and what names it in
the same way, and so on down. It is settled when the relation is next brought up to date
(each frame, in `PostUpdate`), not at the moment of the despawn. `Parent` keeps: use
`despawn_with_related::<Parent>` to take a whole tree at once.

## Naming several

A component that holds a list relates its entity to each of them:

```rust
struct Likes(Vec<Entity>);

impl Relation for Likes {
    fn target(&self) -> Entity {
        self.0[0]
    }
    fn each_target(&self, each: &mut dyn FnMut(Entity)) {
        self.0.iter().copied().for_each(each);
    }
    const WHEN_TARGET_GOES: WhenTargetGoes = WhenTargetGoes::Unlink;
    fn unlink(&mut self, gone: Entity) -> bool {
        self.0.retain(|liked| *liked != gone);
        !self.0.is_empty()
    }
}
```

`Related<Likes>` on an entity is then everyone who likes it. Under `Unlink`, `unlink` takes
the one name out of the list and says whether the component is still worth keeping.

## What isn't here yet

- Cleanup waits for the relation to be brought up to date; nothing runs at the despawn.
- Relations defined by plugins through the C interface, and relations shown as such over
  the debug connection (they are visible as components, if registered).
