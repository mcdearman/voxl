# Credits

Assets used by `examples/napoleonic`. Everything here is free to use; the MIT-licensed items
need their notices kept (they're alongside the files).

| What | Where | Licence |
|---|---|---|
| Sky: *Kloofendal 48d Partly Cloudy (Pure Sky)*, 4K HDR | [Poly Haven](https://polyhaven.com/a/kloofendal_48d_partly_cloudy_puresky) | CC0 |
| Ground and building materials: aerial_grass_rock, farm_soil, grass_path_2, forest_leaves_02, brown_mud, old_stone_wall, plaster_stone_wall_01, roof_slates_02, roof_tiles, oak_wood_planks (2K) | [Poly Haven](https://polyhaven.com/textures) | CC0 |
| Soldiers: *Male_Adult_08*, converted to glTF, then dressed and posed by `tools/soldiers.py` | [Microsoft Rocketbox](https://github.com/microsoft/Microsoft-Rocketbox) | MIT, `models/army/LICENSE_MIT_Microsoft_Rocketbox.txt` |
| Gun crew motion: idles, walk and run captured from life (`m_idle_neutral_01/02`, `m_walk_neutral_01`, `m_run_neutral_01`, kept in `res/paris/sources/rocketbox_animations`), retargeted by `tools/soldiers.py` (`ONLY=crew`); the drill itself is keyed by hand | [Microsoft Rocketbox](https://github.com/microsoft/Microsoft-Rocketbox) | MIT |
| Horse: rigged draught horse (mesh by Lyndon Daniels, rig by ChadM), re-materialled and rescaled | [OpenGameArt](https://opengameart.org/content/rigged-horse) | CC0 |
| Trees and hedge bushes: generated with EZ-Tree (presets Oak Large, Ash Medium, Aspen Large tuned as a Lombardy poplar, Bush 1); levels of detail by `tools/trees.py` | [EZ-Tree](https://github.com/dgreenheck/ez-tree) | MIT, `models/trees/LICENSE_MIT_EZ-Tree.txt`; bark textures from Poly Haven (CC0) |

Made for the demo: the straw, glass and canvas materials (`materials.rs`), the smoke puffs
(`smoke.rs`), the uniforms painted onto the figures, and all the buildings, guns and saddlery.

`sources/` holds the figure and horse as they were before `tools/soldiers.py` runs, so the
army can be rebuilt:

```
blender -b --python examples/napoleonic/tools/soldiers.py -- \
    res/napoleonic/sources/human_rocketbox_male08/Male_Adult_08.gltf \
    res/napoleonic/sources/horse_rigged_ogat/horse.gltf \
    res/napoleonic/models/army
```
