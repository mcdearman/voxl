{-# LANGUAGE ForeignFunctionInterface #-}

-- | A whole (very small) game as a voxl plugin: steer the cube with WASD or the arrow keys
-- and collect the spheres. Each one collected is replaced somewhere else.
--
-- > plugins/chase/build.sh && cargo run --example host -- chase
--
-- The host knows nothing about the game. Everything here can be changed while it is being
-- played: edit, run the build script again, and carry on from the same position and score.
module Chase where

import Control.Monad (forM_, when)
import Data.Bits (shiftR)
import Data.Int (Int32)
import Data.Word (Word32)
import Foreign (Ptr, Storable (..), castPtr)
import Foreign.C.Types (CInt (..))
import Voxl

-- Tuning. Change these while the game is running.
playerSpeed, reach, fieldSize :: Float
playerSpeed = 7
reach = 1.1
fieldSize = 9

pickupCount :: Int
pickupCount = 10

-- | Marks the cube the player steers.
newtype Player = Player Float

instance Storable Player where
  sizeOf _ = 4
  alignment _ = 4
  peek ptr = Player <$> peek (castPtr ptr)
  poke ptr (Player speed) = poke (castPtr ptr) speed

-- | Marks something to collect; the number staggers its bobbing.
newtype Pickup = Pickup Float

instance Storable Pickup where
  sizeOf _ = 4
  alignment _ = 4
  peek ptr = Pickup <$> peek (castPtr ptr)
  poke ptr (Pickup phase) = poke (castPtr ptr) phase

-- | What the systems share. The pointers are to blocks the engine keeps across reloads.
data Game = Game
  { transformC :: Component Transform
  , playerC :: Component Player
  , pickupC :: Component Pickup
  , ballMesh :: Ptr Mesh
  , score :: Ptr Int32
  , seed :: Ptr Word32
  }

foreign export ccall voxl_hs_main :: Ptr () -> IO CInt

voxl_hs_main :: Ptr () -> IO CInt
voxl_hs_main = plugin $ \app -> do
  game <-
    Game
      <$> lookupComponent app "voxl.Transform"
      <*> registerComponent app "chase.Player"
      <*> registerComponent app "chase.Pickup"
      <*> statePtr app "chase.ball"
      <*> statePtr app "chase.score"
      <*> statePtr app "chase.seed"
  let transform = transformC game

  -- Runs once, however many times the plugin is reloaded.
  addSystem_ app "setup" Startup (setup game)

  addSystem app "steer" Update (write transform <* with (playerC game)) $ \sys _ place -> do
    dt <- deltaSeconds sys
    let held keys = or <$> mapM (keyDown sys) keys
        axis negative positive = do
          back <- held negative
          forth <- held positive
          pure (fromIntegral (fromEnum forth - fromEnum back))
    dx <- axis [KeyA, ArrowLeft] [KeyD, ArrowRight]
    dz <- axis [KeyW, ArrowUp] [KeyS, ArrowDown]
    let len = max 1 (sqrt (dx * dx + dz * dz))
        step = playerSpeed * dt / len
    modifyRef place $ \t ->
      let V3 x y z = translation t
          keep = max (-fieldSize) . min fieldSize
       in t {translation = V3 (keep (x + dx * step)) y (keep (z + dz * step))}

  -- Two queries: where the players are, and where the pickups are. Both only read.
  addSystem2 app "collect" Update
    (readC transform <* with (playerC game))
    (readC transform <* with (pickupC game))
    $ \sys players pickups ->
      forEach players $ \_ player ->
        forEach pickups $ \entity pickup ->
          when (flatDistance player pickup < reach) $ do
            despawn sys entity
            total <- (+ 1) <$> peek (score game)
            poke (score game) total
            logInfo ("collected " ++ show total)
            spawnPickup game sys

  addSystem app "bob" Update ((,) <$> write transform <*> readC (pickupC game)) $
    \sys _ (place, Pickup phase) -> do
      time <- realToFrac <$> elapsedSeconds sys
      modifyRef place $ \t ->
        let V3 x _ z = translation t
         in t {translation = V3 x (0.6 + 0.2 * sin (time * 3 + phase)) z}

flatDistance :: Transform -> Transform -> Float
flatDistance a b =
  let V3 ax _ az = translation a
      V3 bx _ bz = translation b
   in sqrt ((ax - bx) ^ (2 :: Int) + (az - bz) ^ (2 :: Int))

setup :: Game -> System -> IO ()
setup game sys = do
  cube <- meshCube sys 1
  ball <- meshSphere sys 0.4
  ground <- meshPlane sys 1
  poke (ballMesh game) ball

  field <- spawn sys
  insert sys field (transformC game) (at (V3 0 0 0)) {scale = V3 (fieldSize * 2 + 2) 1 (fieldSize * 2 + 2)}
  setMesh sys field ground
  setMaterial sys field (material (V3 0.16 0.30 0.20))

  player <- spawn sys
  insert sys player (transformC game) (at (V3 0 0.5 0))
  insert sys player (playerC game) (Player playerSpeed)
  setMesh sys player cube
  setMaterial sys player (material (V3 0.85 0.25 0.20))

  forM_ [1 .. pickupCount] $ \_ -> spawnPickup game sys

at :: V3 -> Transform
at position = Transform position (Quat 0 0 0 1) (V3 1 1 1)

spawnPickup :: Game -> System -> IO ()
spawnPickup game sys = do
  x <- random game
  z <- random game
  phase <- random game
  ball <- peek (ballMesh game)
  entity <- spawn sys
  insert sys entity (transformC game) (at (V3 (x * fieldSize) 0.6 (z * fieldSize)))
  insert sys entity (pickupC game) (Pickup (phase * 3))
  setMesh sys entity ball
  setMaterial sys entity (material (V3 1.0 0.78 0.15)) {emissive = V3 0.5 0.35 0.05, roughness = 0.3}

-- | A number from -1 to 1. The generator's state is in the engine, so the sequence carries on
-- across reloads instead of starting again.
random :: Game -> IO Float
random game = do
  s <- peek (seed game)
  let s' = s * 1664525 + 1013904223
  poke (seed game) s'
  pure (fromIntegral (s' `shiftR` 8) / 8388608 - 1)
