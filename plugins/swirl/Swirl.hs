{-# LANGUAGE ForeignFunctionInterface #-}

-- | An example voxl plugin in Haskell: turns the grid of cubes in the `plugins` example
-- about its centre, a ring at a time.
--
-- Build it with @plugins/swirl/build.sh@, run @cargo run --example plugins@, then change a
-- number below and build again: the running app picks up the new code, and every cube keeps
-- its 'Orbit' (how far round it has got) across the reload.
module Swirl where

import Foreign (Ptr, Storable (..), castPtr, peekElemOff, pokeElemOff)
import Foreign.C.Types (CInt (..))
import Voxl

-- | Turns per second at the centre; rings further out lag behind.
speed :: Float
speed = 0.15

-- | Defined by the host: where a cube sits on the grid.
data Cell = Cell !Float !Float

instance Storable Cell where
  sizeOf _ = 8
  alignment _ = 4
  peek ptr = Cell <$> peekElemOff (castPtr ptr) 0 <*> peekElemOff (castPtr ptr) 1
  poke ptr (Cell x z) = pokeElemOff (castPtr ptr) 0 x >> pokeElemOff (castPtr ptr) 1 z

-- | Defined here: the angle a cube has turned through. It lives in the engine, so it carries
-- on from where it was when the plugin is reloaded.
newtype Orbit = Orbit Float

instance Storable Orbit where
  sizeOf _ = 4
  alignment _ = 4
  peek ptr = Orbit <$> peek (castPtr ptr)
  poke ptr (Orbit angle) = poke (castPtr ptr) angle

foreign export ccall "voxl_hs_main" pluginMain :: Ptr () -> IO CInt

pluginMain :: Ptr () -> IO CInt
pluginMain = plugin $ \app -> do
  transform <- lookupComponent app "voxl.Transform"
  cell <- lookupComponent app "demo.Cell"
  orbit <- registerComponent app "swirl.Orbit"

  loads <- statePtr app "swirl.loads" :: IO (Ptr CInt)
  count <- (+ 1) <$> peek loads
  poke loads count
  logInfo ("swirl loaded (" ++ show count ++ " time(s) this run)")

  -- Give every cell that has no orbit yet one that starts at zero.
  addSystem app "adopt" Update (with cell <* without orbit) $ \sys entity () ->
    insert sys entity orbit (Orbit 0)

  addSystem app "turn" Update ((,,) <$> write transform <*> readC cell <*> write orbit) $
    \sys _entity (place, Cell x z, turned) -> do
      dt <- deltaSeconds sys
      Orbit angle <- readRef turned
      let radius = sqrt (x * x + z * z)
          angle' = angle + dt * speed * 2 * pi / (1 + radius * 0.15)
          (s, c) = (sin angle', cos angle')
          spacing = 1.3
      writeRef turned (Orbit angle')
      modifyRef place $ \t ->
        let V3 _ y _ = translation t
         in t {translation = V3 ((x * c - z * s) * spacing) y ((x * s + z * c) * spacing)}
