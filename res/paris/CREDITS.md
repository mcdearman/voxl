# Credits

Assets used by `examples/paris`. Everything here is free to use; the MIT-licensed items need
their notice kept (`LICENSE_MIT_Microsoft_Rocketbox.txt`, alongside this file).

| What | Where | Licence |
|---|---|---|
| Sky: *Kloofendal 48d Partly Cloudy (Pure Sky)*, 4K HDR | [Poly Haven](https://polyhaven.com/a/kloofendal_48d_partly_cloudy_puresky) | CC0 |
| Materials: cobblestone_pavement, sandstone_blocks_08, beige_wall_001, wood_shutter, rusty_painted_metal, roof_slates_02 (4K, shrunk to 2K on loading), red_brick_03 (2K) | [Poly Haven](https://polyhaven.com/textures) | CC0 |
| Cloth for the costumes: rough_linen, poly_wool_herringbone, fabric_leather_01 | [Poly Haven](https://polyhaven.com/textures) | CC0 |
| People: *Male_Adult_02, 04, 07, 11, 13, 15* and *Female_Adult_01, 06, 09, 12*, dressed, posed and exported by `tools/people.py` | [Microsoft Rocketbox](https://github.com/microsoft/Microsoft-Rocketbox) | MIT |
| Animations: walks, idles and conversation gestures (`m_/f_walk_neutral_01`, `m_walk_slow_01`, `f_walk_stroll_01`, `*_idle_neutral_01/02`, `*_gestic_talk_neutral_01/02`, `*_gestic_talk_relaxed_01`), retargeted onto each figure by `tools/rig.py` | [Microsoft Rocketbox](https://github.com/microsoft/Microsoft-Rocketbox) | MIT |
| Trees along the quay (`models/trees/ash.glb`), from the Napoleonic demo: generated with EZ-Tree | [EZ-Tree](https://github.com/dgreenheck/ez-tree) | MIT, `models/trees/LICENSE_MIT_EZ-Tree.txt` |
| Children: *Male_Child_01, 02* and *Female_Child_01, 02*, dressed by `tools/people.py`; runs `m_/f_run_neutral_01` | [Microsoft Rocketbox](https://github.com/microsoft/Microsoft-Rocketbox) | MIT |
| Animals: *Horse_Brown_01, Horse_LightBrown_01, Pig_Pink_01, Dog_Beagle_01, Dog_GermanShepard_01, Bird_Chicken_Brown_01, Bird_Chicken_White_01, Bird_Rooster_Brown_01, Bird_Goose_White_01*, their gaits and idles animated by `tools/animals.py` | [Microsoft Rocketbox](https://github.com/microsoft/Microsoft-Rocketbox) | MIT |

Made for the demo: the square, its houses, the fountain, the carts and coach (`square.rs`,
`buildings.rs`), and the period costumes painted and modelled onto the figures: coats,
waistcoats, gowns, skirts and aprons, top hats, round hats, bicornes, straw hats and caps, the
statues round the column and the gilded Victory on top.

The scene follows Étienne Bouhot's painting *La Fontaine et la place du Châtelet* (1810,
Musée Carnavalet), which is in the public domain.

`sources/rocketbox` holds the figures as downloaded (their textures converted to PNG), and
`sources/rocketbox_animations` the animation files, so the people can be rebuilt (each takes
about a minute, most of it retargeting the animations):

```
cd examples/paris/tools
blender -b --python people.py -- ../../../res/paris/sources/rocketbox ../../../res/paris/textures ../../../res/paris/models
```

Set `PEOPLE=gentleman,maid,...` to rebuild only some of them.
