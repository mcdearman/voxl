{-# LANGUAGE ForeignFunctionInterface #-}
{-# LANGUAGE ScopedTypeVariables #-}

-- | Bindings for writing voxl plugins in Haskell.
--
-- A plugin is a module that exports @voxl_hs_main@, built into a shared library together with
-- this module and @cbits/voxl_hs.c@ (see @plugins/swirl@ and its build script):
--
-- > foreign export ccall voxl_hs_main :: Ptr () -> IO CInt
-- > voxl_hs_main :: Ptr () -> IO CInt
-- > voxl_hs_main = plugin $ \app -> do
-- >   transform <- lookupComponent app "voxl.Transform"
-- >   addSystem app "rise" Update (write transform) $ \sys _entity ref -> do
-- >     dt <- deltaSeconds sys
-- >     modifyRef ref $ \t -> t { translation = translation t + V3 0 dt 0 }
--
-- A system's query is built from 'readC', 'write', 'with' and 'without', combined
-- applicatively. The query decides both which entities the system visits and what the
-- system's function is handed for each one, so there are no pointers to cast and no way to
-- read a component the query didn't ask for.
--
-- The plugin is reloaded when its library is rebuilt. Component values and 'statePtr' blocks
-- survive; top-level 'IORef's and other Haskell values do not, because a reload is a fresh
-- copy of the program.
module Voxl
  ( -- * The plugin
    App
  , plugin
    -- * Components
  , Component
  , registerComponent
  , lookupComponent
  , statePtr
    -- * Queries
  , Query
  , readC
  , write
  , with
  , without
  , Ref
  , readRef
  , writeRef
  , modifyRef
    -- * Systems
  , System
  , Stage (..)
  , Entity
  , addSystem
  , addSystem_
  , Each
  , addSystem1
  , addSystem2
  , addSystem3
  , forEach
  , fetch
  , deltaSeconds
  , elapsedSeconds
  , spawn
  , despawn
  , insert
  , remove
    -- * Input
  , Key (..)
  , MouseButton (..)
  , keyDown
  , keyPressed
  , keyReleased
  , mouseDown
  , mousePressed
  , mouseMotion
    -- * Drawing
  , Mesh
  , Vertex (..)
  , Material (..)
  , material
  , meshCube
  , meshSphere
  , meshPlane
  , meshCreate
  , setMesh
  , setMaterial
    -- * Logging
  , logInfo
  , logWarn
  , logError
    -- * Engine components
  , Transform (..)
  , V3 (..)
  , Quat (..)
  ) where

import Control.Exception (SomeException, displayException, try)
import Control.Monad (unless, when)
import Data.IORef (IORef, atomicModifyIORef', newIORef)
import Data.Word (Word32, Word64, Word8)
import Foreign
  ( Ptr
  , Storable (..)
  , alloca
  , allocaArray
  , castPtr
  , peekElemOff
  , plusPtr
  , pokeElemOff
  , withArrayLen
  )
import qualified Foreign
import Foreign.C.String (withCStringLen)
import Foreign.C.Types (CChar, CDouble (..), CFloat (..), CInt (..), CSize (..))
import Foreign.StablePtr (StablePtr, castPtrToStablePtr, castStablePtrToPtr, deRefStablePtr, freeStablePtr, newStablePtr)
import System.IO.Unsafe (unsafePerformIO)

-- The engine's functions, by way of cbits/voxl_hs.c. The ones called once per entity are
-- imported `unsafe`, which makes them as cheap as a C call; none of them calls back.

foreign import ccall unsafe "voxl_hs_log" c_log :: Word32 -> Ptr CChar -> CSize -> IO ()
foreign import ccall unsafe "voxl_hs_component_register"
  c_component_register :: Ptr () -> Ptr CChar -> CSize -> CSize -> CSize -> IO Word32
foreign import ccall unsafe "voxl_hs_component_lookup"
  c_component_lookup :: Ptr () -> Ptr CChar -> CSize -> Ptr CSize -> Ptr CSize -> IO Word32
foreign import ccall unsafe "voxl_hs_state"
  c_state :: Ptr () -> Ptr CChar -> CSize -> CSize -> CSize -> IO (Ptr ())
foreign import ccall unsafe "voxl_hs_delta_seconds" c_delta_seconds :: Ptr () -> IO CFloat
foreign import ccall unsafe "voxl_hs_elapsed_seconds" c_elapsed_seconds :: Ptr () -> IO CDouble
foreign import ccall unsafe "voxl_hs_system_add_queries"
  c_system_add_queries ::
    Ptr () -> Ptr CChar -> CSize -> Word32 -> Ptr () -> Ptr Word32 -> Ptr CSize -> CSize -> IO CInt
foreign import ccall unsafe "voxl_hs_query_next_in"
  c_query_next_in :: Ptr () -> Word32 -> Ptr Word64 -> Ptr (Ptr ()) -> IO Word8
foreign import ccall unsafe "voxl_hs_query_get_in"
  c_query_get_in :: Ptr () -> Word32 -> Word64 -> Ptr (Ptr ()) -> IO Word8
foreign import ccall unsafe "voxl_hs_query_rewind" c_query_rewind :: Ptr () -> Word32 -> IO ()
foreign import ccall unsafe "voxl_hs_key_down" c_key_down :: Ptr () -> Word32 -> IO Word8
foreign import ccall unsafe "voxl_hs_key_pressed" c_key_pressed :: Ptr () -> Word32 -> IO Word8
foreign import ccall unsafe "voxl_hs_key_released" c_key_released :: Ptr () -> Word32 -> IO Word8
foreign import ccall unsafe "voxl_hs_mouse_down" c_mouse_down :: Ptr () -> Word32 -> IO Word8
foreign import ccall unsafe "voxl_hs_mouse_pressed" c_mouse_pressed :: Ptr () -> Word32 -> IO Word8
foreign import ccall unsafe "voxl_hs_mouse_motion" c_mouse_motion :: Ptr () -> Ptr CFloat -> IO ()
foreign import ccall unsafe "voxl_hs_mesh_shape" c_mesh_shape :: Ptr () -> Word32 -> CFloat -> IO Word32
foreign import ccall unsafe "voxl_hs_mesh_create"
  c_mesh_create :: Ptr () -> Ptr Vertex -> CSize -> Ptr Word32 -> CSize -> IO Word32
foreign import ccall unsafe "voxl_hs_set_mesh" c_set_mesh :: Ptr () -> Word64 -> Word32 -> IO ()
foreign import ccall unsafe "voxl_hs_set_material" c_set_material :: Ptr () -> Word64 -> Ptr Material -> IO ()
foreign import ccall unsafe "voxl_hs_spawn" c_spawn :: Ptr () -> IO Word64
foreign import ccall unsafe "voxl_hs_despawn" c_despawn :: Ptr () -> Word64 -> IO ()
foreign import ccall unsafe "voxl_hs_insert" c_insert :: Ptr () -> Word64 -> Word32 -> Ptr () -> IO ()
foreign import ccall unsafe "voxl_hs_remove" c_remove :: Ptr () -> Word64 -> Word32 -> IO ()

-- | The app being set up. Only meaningful inside the function given to 'plugin'.
newtype App = App (Ptr ())

-- | One run of a system. Only meaningful inside that system's function.
newtype System = System (Ptr ())

-- | An entity. Valid until it is despawned.
newtype Entity = Entity Word64
  deriving (Eq, Ord, Show)

-- | A component whose values are of type @a@.
newtype Component a = Component Word32

-- | When in the frame a system runs.
data Stage
  = -- | Once, the first time the plugin is loaded (not again on reload).
    Startup
  | First
  | PreUpdate
  | -- | Fixed timestep: zero or more times a frame.
    FixedUpdate
  | Update
  | PostUpdate
  | Last
  deriving (Eq, Show, Enum)

logAt :: Word32 -> String -> IO ()
logAt level message =
  withCStringLen message $ \(chars, len) -> c_log level chars (fromIntegral len)

logError, logWarn, logInfo :: String -> IO ()
logError = logAt 1
logWarn = logAt 2
logInfo = logAt 3

-- | Wraps a plugin's setup function as its entry point. An exception thrown while setting up
-- is logged and makes the load fail, which leaves the previous version (if any) unloaded and
-- the engine running.
plugin :: (App -> IO ()) -> Ptr () -> IO CInt
plugin setup app = do
  result <- try (setup (App app))
  case result of
    Right () -> pure 0
    Left (err :: SomeException) -> do
      logError ("plugin failed to load: " ++ displayException err)
      pure (-1)

withName :: String -> (Ptr CChar -> CSize -> IO a) -> IO a
withName name use = withCStringLen name $ \(chars, len) -> use chars (fromIntegral len)

-- | Defines a component stored as the bytes of @a@, or finds it if this plugin defined it
-- before a reload. Its values are kept across reloads as long as @a@ keeps its size and
-- alignment.
registerComponent :: forall a. Storable a => App -> String -> IO (Component a)
registerComponent (App app) name = do
  let proxy = undefined :: a
  handle <- withName name $ \chars len ->
    c_component_register app chars len (fromIntegral (sizeOf proxy)) (fromIntegral (alignment proxy))
  when (handle == 0) $ ioError (userError ("could not register component `" ++ name ++ "`"))
  pure (Component handle)

-- | Finds a component defined by the engine or another plugin. Fails if there is none, or if
-- @a@ is not the size and alignment the engine has for it.
lookupComponent :: forall a. Storable a => App -> String -> IO (Component a)
lookupComponent (App app) name = do
  let proxy = undefined :: a
  (handle, size, align) <- withName name $ \chars len ->
    alloca $ \sizeOut -> alloca $ \alignOut -> do
      handle <- c_component_lookup app chars len sizeOut alignOut
      (,,) handle <$> peek sizeOut <*> peek alignOut
  when (handle == 0) $ ioError (userError ("no component named `" ++ name ++ "`"))
  unless (fromIntegral size == sizeOf proxy && fromIntegral align == alignment proxy) $
    ioError . userError $
      "component `" ++ name ++ "` is " ++ show size ++ " bytes aligned to " ++ show align
        ++ ", but the plugin's type is " ++ show (sizeOf proxy) ++ " bytes aligned to "
        ++ show (alignment proxy)
  pure (Component handle)

-- | A block of memory owned by the engine, zeroed when first asked for, that survives
-- reloads. This is where a plugin keeps anything a top-level 'IORef' would otherwise hold.
statePtr :: forall a. Storable a => App -> String -> IO (Ptr a)
statePtr (App app) name = do
  let proxy = undefined :: a
  castPtr <$> withName name (\chars len ->
    c_state app chars len (fromIntegral (sizeOf proxy)) (fromIntegral (alignment proxy)))

-- | What a system asks of each entity it visits, and so what its function receives.
--
-- > (,) <$> write transform <*> readC velocity <* without frozen
data Query a = Query
  { queryTerms :: [(Word32, Word32)]
    -- ^ component handle and access mode, in order
  , querySlots :: Int
    -- ^ how many pointers the engine hands back per entity
  , queryDecode :: Ptr (Ptr ()) -> Int -> IO a
    -- ^ builds the result from those pointers, starting at the given slot
  }

instance Functor Query where
  fmap f (Query terms slots decode) = Query terms slots (\found at -> f <$> decode found at)

instance Applicative Query where
  pure x = Query [] 0 (\_ _ -> pure x)
  Query termsF slotsF decodeF <*> Query termsX slotsX decodeX =
    Query
      (termsF ++ termsX)
      (slotsF + slotsX)
      (\found at -> decodeF found at <*> decodeX found (at + slotsF))

-- | Visit entities that have the component, and receive its value.
readC :: Storable a => Component a -> Query a
readC (Component handle) =
  Query [(handle, 0)] 1 (\found at -> peekElemOff found at >>= peek . castPtr)

-- | A component of the entity being visited, which the system may change.
newtype Ref a = Ref (Ptr a)

-- | Visit entities that have the component, and receive a reference to change it through.
write :: Component a -> Query (Ref a)
write (Component handle) =
  Query [(handle, 1)] 1 (\found at -> Ref . castPtr <$> peekElemOff found at)

-- | Only visit entities that have the component.
with :: Component a -> Query ()
with (Component handle) = Query [(handle, 2)] 0 (\_ _ -> pure ())

-- | Only visit entities that don't have the component.
without :: Component a -> Query ()
without (Component handle) = Query [(handle, 3)] 0 (\_ _ -> pure ())

readRef :: Storable a => Ref a -> IO a
readRef (Ref ptr) = peek ptr

writeRef :: Storable a => Ref a -> a -> IO ()
writeRef (Ref ptr) = poke ptr

modifyRef :: Storable a => Ref a -> (a -> a) -> IO ()
modifyRef ref f = readRef ref >>= writeRef ref . f

-- Every system's Haskell function, kept alive for the engine to call. They are released when
-- this version of the plugin is unloaded, so the collector can let go of what they captured.
{-# NOINLINE systems #-}
systems :: IORef [StablePtr (Ptr () -> IO ())]
systems = unsafePerformIO (newIORef [])

-- | One of a system's queries, ready to be walked while the system runs.
data Each a = Each (Ptr ()) Word32 (Query a)

-- | Calls the function for every entity the query matches. Calling it again starts over.
-- (Walking the same 'Each' inside its own 'forEach' restarts the outer walk too; give the
-- system two queries instead.)
forEach :: Each a -> (Entity -> a -> IO ()) -> IO ()
forEach (Each system index query) visit = do
  c_query_rewind system index
  allocaArray (max 1 (querySlots query)) $ \found -> alloca $ \entityOut ->
    let loop = do
          more <- c_query_next_in system index entityOut found
          when (more /= 0) $ do
            entity <- peek entityOut
            item <- queryDecode query found 0
            visit (Entity entity) item
            loop
     in loop

-- | Looks one entity up. 'Nothing' if the query doesn't match it.
fetch :: Each a -> Entity -> IO (Maybe a)
fetch (Each system index query) (Entity entity) =
  allocaArray (max 1 (querySlots query)) $ \found -> do
    matched <- c_query_get_in system index entity found
    if matched /= 0 then Just <$> queryDecode query found 0 else pure Nothing

-- Registers a system whose queries have these term lists, to run `body`.
register :: App -> String -> Stage -> [[(Word32, Word32)]] -> (Ptr () -> IO ()) -> IO ()
register (App app) name stage termLists body = do
  closure <- newStablePtr body
  atomicModifyIORef' systems (\held -> (closure : held, ()))
  let flat = concat [[handle, access] | terms <- termLists, (handle, access) <- terms]
      counts = map (fromIntegral . length) termLists :: [CSize]
  status <- withName name $ \chars len ->
    withArrayLen flat $ \_ terms -> withArrayLen counts $ \queries countsPtr ->
      c_system_add_queries app chars len (fromIntegral (fromEnum stage)) (castStablePtrToPtr closure)
        terms countsPtr (fromIntegral queries)
  when (status /= 0) $
    ioError . userError $
      "could not add system `" ++ name ++ "` (see the engine's log; two queries of one system "
        ++ "may not reach the same component unless neither writes it or `with`/`without` keep "
        ++ "them apart)"

-- | Adds a system that visits every entity matching the query, calling the function once for
-- each with the entity and what the query asked for.
addSystem :: App -> String -> Stage -> Query a -> (System -> Entity -> a -> IO ()) -> IO ()
addSystem app name stage query visit =
  addSystem1 app name stage query $ \sys each -> forEach each (visit sys)

-- | Adds a system that runs once each time its stage comes round, visiting nothing.
addSystem_ :: App -> String -> Stage -> (System -> IO ()) -> IO ()
addSystem_ app name stage body = register app name stage [[]] (body . System)

-- | Adds a system that runs once each time its stage comes round, with a query to walk
-- ('forEach') or look entities up in ('fetch') as it chooses.
addSystem1 :: App -> String -> Stage -> Query a -> (System -> Each a -> IO ()) -> IO ()
addSystem1 app name stage qa body =
  register app name stage [queryTerms qa] $ \sys -> body (System sys) (Each sys 0 qa)

-- | As 'addSystem1', with two queries: say, the player and the things it can pick up. The
-- engine refuses the system if both queries could reach the same component of the same
-- entity and either writes it; 'with' and 'without' are how to keep them apart.
addSystem2 ::
  App -> String -> Stage -> Query a -> Query b -> (System -> Each a -> Each b -> IO ()) -> IO ()
addSystem2 app name stage qa qb body =
  register app name stage [queryTerms qa, queryTerms qb] $ \sys ->
    body (System sys) (Each sys 0 qa) (Each sys 1 qb)

addSystem3 ::
  App ->
  String ->
  Stage ->
  Query a ->
  Query b ->
  Query c ->
  (System -> Each a -> Each b -> Each c -> IO ()) ->
  IO ()
addSystem3 app name stage qa qb qc body =
  register app name stage [queryTerms qa, queryTerms qb, queryTerms qc] $ \sys ->
    body (System sys) (Each sys 0 qa) (Each sys 1 qb) (Each sys 2 qc)

-- | Seconds since the last frame (or the fixed timestep, in 'FixedUpdate').
deltaSeconds :: System -> IO Float
deltaSeconds (System system) = realToFrac <$> c_delta_seconds system

-- | Seconds since the app started.
elapsedSeconds :: System -> IO Double
elapsedSeconds (System system) = realToFrac <$> c_elapsed_seconds system

-- | Creates an entity. It can be given components at once; it shows up in queries after this
-- system returns.
spawn :: System -> IO Entity
spawn (System system) = Entity <$> c_spawn system

despawn :: System -> Entity -> IO ()
despawn (System system) (Entity entity) = c_despawn system entity

-- | Adds or replaces a component when this system returns.
insert :: Storable a => System -> Entity -> Component a -> a -> IO ()
insert (System system) (Entity entity) (Component handle) value =
  Foreign.with value $ \ptr -> c_insert system entity handle (castPtr ptr)

remove :: System -> Entity -> Component a -> IO ()
remove (System system) (Entity entity) (Component handle) = c_remove system entity handle

-- | A key, by its position on a US keyboard (so 'KeyW' is the same key on every layout).
data Key
  = KeyA | KeyB | KeyC | KeyD | KeyE | KeyF | KeyG | KeyH | KeyI | KeyJ | KeyK | KeyL | KeyM
  | KeyN | KeyO | KeyP | KeyQ | KeyR | KeyS | KeyT | KeyU | KeyV | KeyW | KeyX | KeyY | KeyZ
  | Digit0 | Digit1 | Digit2 | Digit3 | Digit4 | Digit5 | Digit6 | Digit7 | Digit8 | Digit9
  | Space | Enter | Escape | Tab | Backspace
  | ArrowLeft | ArrowRight | ArrowUp | ArrowDown
  | ShiftLeft | ShiftRight | ControlLeft | ControlRight | AltLeft | AltRight
  | F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12
  deriving (Eq, Show, Enum, Bounded)

data MouseButton = MouseLeft | MouseRight | MouseMiddle
  deriving (Eq, Show, Enum, Bounded)

asked :: (Ptr () -> Word32 -> IO Word8) -> Enum k => System -> k -> IO Bool
asked ask (System system) k = (/= 0) <$> ask system (fromIntegral (fromEnum k))

-- | Whether the key is held down.
keyDown :: System -> Key -> IO Bool
keyDown = asked c_key_down

-- | Whether the key went down this frame.
keyPressed :: System -> Key -> IO Bool
keyPressed = asked c_key_pressed

-- | Whether the key came up this frame.
keyReleased :: System -> Key -> IO Bool
keyReleased = asked c_key_released

mouseDown :: System -> MouseButton -> IO Bool
mouseDown = asked c_mouse_down

mousePressed :: System -> MouseButton -> IO Bool
mousePressed = asked c_mouse_pressed

-- | How far the mouse moved this frame, in pixels: x, then y.
mouseMotion :: System -> IO (Float, Float)
mouseMotion (System system) = allocaArray 2 $ \delta -> do
  c_mouse_motion system delta
  (,) <$> (realToFrac <$> peekElemOff delta 0) <*> (realToFrac <$> peekElemOff delta 1)

-- | A mesh, shared between the entities drawn with it. Make it once, in a 'Startup' system,
-- and keep it in a 'statePtr' block: it is 'Storable', and still good after a reload.
newtype Mesh = Mesh Word32
  deriving (Eq, Show)

instance Storable Mesh where
  sizeOf _ = 4
  alignment _ = 4
  peek ptr = Mesh <$> peek (castPtr ptr)
  poke ptr (Mesh handle) = poke (castPtr ptr) handle

-- | One corner of a mesh: position, normal and texture coordinates.
data Vertex = Vertex !V3 !V3 !Float !Float

instance Storable Vertex where
  sizeOf _ = 32
  alignment _ = 4
  peek ptr =
    Vertex <$> peek (castPtr ptr) <*> peek (castPtr (ptr `plusPtr` 12))
      <*> peek (castPtr (ptr `plusPtr` 24)) <*> peek (castPtr (ptr `plusPtr` 28))
  poke ptr (Vertex position normal u v) = do
    poke (castPtr ptr) position
    poke (castPtr (ptr `plusPtr` 12)) normal
    poke (castPtr (ptr `plusPtr` 24)) u
    poke (castPtr (ptr `plusPtr` 28)) v

-- | How a surface looks. Colours are linear red, green and blue.
data Material = Material
  { color :: !V3
  , opacity :: !Float
  , emissive :: !V3
    -- ^ light the surface gives off itself
  , roughness :: !Float
    -- ^ 0 mirror-smooth to 1 matte
  , metallic :: !Float
  }
  deriving (Eq, Show)

-- | A plain, matte surface of the given colour.
material :: V3 -> Material
material rgb = Material rgb 1 0 0.8 0

instance Storable Material where
  sizeOf _ = 36
  alignment _ = 4
  peek ptr =
    Material <$> peek (castPtr ptr) <*> peek (castPtr (ptr `plusPtr` 12))
      <*> peek (castPtr (ptr `plusPtr` 16)) <*> peek (castPtr (ptr `plusPtr` 28))
      <*> peek (castPtr (ptr `plusPtr` 32))
  poke ptr (Material rgb alpha glow rough metal) = do
    poke (castPtr ptr) rgb
    poke (castPtr (ptr `plusPtr` 12)) alpha
    poke (castPtr (ptr `plusPtr` 16)) glow
    poke (castPtr (ptr `plusPtr` 28)) rough
    poke (castPtr (ptr `plusPtr` 32)) metal

made :: String -> Word32 -> IO Mesh
made what handle = do
  when (handle == 0) $ ioError (userError ("could not make " ++ what ++ " (does the app have a renderer?)"))
  pure (Mesh handle)

-- | A cube with the given edge length.
meshCube :: System -> Float -> IO Mesh
meshCube (System system) size = c_mesh_shape system 0 (realToFrac size) >>= made "a cube mesh"

meshSphere :: System -> Float -> IO Mesh
meshSphere (System system) radius = c_mesh_shape system 1 (realToFrac radius) >>= made "a sphere mesh"

-- | A flat square facing up, with the given edge length.
meshPlane :: System -> Float -> IO Mesh
meshPlane (System system) size = c_mesh_shape system 2 (realToFrac size) >>= made "a plane mesh"

-- | A mesh from triangles: three indices each, counter-clockwise seen from the front.
meshCreate :: System -> [Vertex] -> [Word32] -> IO Mesh
meshCreate (System system) vertices indices =
  withArrayLen vertices $ \vertexCount vertexPtr -> withArrayLen indices $ \indexCount indexPtr ->
    c_mesh_create system vertexPtr (fromIntegral vertexCount) indexPtr (fromIntegral indexCount)
      >>= made "a mesh"

-- | Draws the entity as the mesh (it also needs a 'Transform'), when this system returns.
setMesh :: System -> Entity -> Mesh -> IO ()
setMesh (System system) (Entity entity) (Mesh handle) = c_set_mesh system entity handle

-- | Sets the entity's surface, when this system returns.
setMaterial :: System -> Entity -> Material -> IO ()
setMaterial (System system) (Entity entity) surface =
  Foreign.with surface (c_set_material system entity)

-- What the engine calls for every Haskell system: `user` is the stable pointer to its
-- function. An exception must not escape into the engine, so it is logged instead.
foreign export ccall voxl_hs_dispatch :: Ptr () -> Ptr () -> IO ()

voxl_hs_dispatch :: Ptr () -> Ptr () -> IO ()
voxl_hs_dispatch system user = do
  run <- deRefStablePtr (castPtrToStablePtr user)
  result <- try (run system)
  case result of
    Right () -> pure ()
    Left (err :: SomeException) -> logError ("a system threw: " ++ displayException err)

foreign export ccall voxl_hs_unload :: IO ()

voxl_hs_unload :: IO ()
voxl_hs_unload = atomicModifyIORef' systems (\held -> ([], held)) >>= mapM_ freeStablePtr

-- | A vector of three floats.
data V3 = V3 !Float !Float !Float
  deriving (Eq, Show)

instance Num V3 where
  V3 a b c + V3 x y z = V3 (a + x) (b + y) (c + z)
  V3 a b c - V3 x y z = V3 (a - x) (b - y) (c - z)
  V3 a b c * V3 x y z = V3 (a * x) (b * y) (c * z)
  abs (V3 a b c) = V3 (abs a) (abs b) (abs c)
  signum (V3 a b c) = V3 (signum a) (signum b) (signum c)
  fromInteger n = V3 (fromInteger n) (fromInteger n) (fromInteger n)

instance Storable V3 where
  sizeOf _ = 12
  alignment _ = 4
  peek ptr = V3 <$> peekElemOff (castPtr ptr) 0 <*> peekElemOff (castPtr ptr) 1 <*> peekElemOff (castPtr ptr) 2
  poke ptr (V3 x y z) = pokeElemOff (castPtr ptr) 0 x >> pokeElemOff (castPtr ptr) 1 y >> pokeElemOff (castPtr ptr) 2 z

-- | A unit quaternion: x, y, z, w.
data Quat = Quat !Float !Float !Float !Float
  deriving (Eq, Show)

-- | @voxl.Transform@: position, rotation and scale relative to the entity's parent. Laid out
-- as @VoxlTransform@ in @voxl.h@: 48 bytes, 16-byte aligned.
data Transform = Transform
  { translation :: !V3
  , rotation :: !Quat
  , scale :: !V3
  }
  deriving (Eq, Show)

instance Storable Transform where
  sizeOf _ = 48
  alignment _ = 16
  peek ptr = do
    t <- peek (castPtr ptr)
    let q = castPtr (ptr `plusPtr` 16) :: Ptr Float
    r <- Quat <$> peekElemOff q 0 <*> peekElemOff q 1 <*> peekElemOff q 2 <*> peekElemOff q 3
    s <- peek (castPtr (ptr `plusPtr` 32))
    pure (Transform t r s)
  poke ptr (Transform t (Quat x y z w) s) = do
    poke (castPtr ptr) t
    let q = castPtr (ptr `plusPtr` 16) :: Ptr Float
    pokeElemOff q 0 x >> pokeElemOff q 1 y >> pokeElemOff q 2 z >> pokeElemOff q 3 w
    poke (castPtr (ptr `plusPtr` 32)) s
