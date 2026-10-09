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

## What isn't here yet

- One `R` per entity: an entity is `ShotBy` one archer. Many of the same kind (likes
  several others) needs a component that holds a list, which `Relation` does not cover yet.
- Nothing happens by itself to the sources when a target is despawned; ask for it with
  `despawn_with_related`.
- Relations defined by plugins through the C interface, and relations shown as such over
  the debug connection (they are visible as components, if registered).
