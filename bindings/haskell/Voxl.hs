{-# LANGUAGE ForeignFunctionInterface #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# LANGUAGE TupleSections #-}

-- | Bindings for writing voxl plugins in Haskell.
--
-- A plugin is a module that exports its setup function to C as @voxl_hs_main@ (it can have
-- any Haskell name), built into a shared library together with
-- this module and @cbits/voxl_hs.c@ (see @plugins/swirl@ and its build script):
--
-- > foreign export ccall "voxl_hs_main" pluginMain :: Ptr () -> IO CInt
-- > pluginMain :: Ptr () -> IO CInt
-- > pluginMain = plugin $ \app -> do
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
  , FieldType (..)
  , Field (..)
  , describeComponent
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
  , despawnTree
  , setParent
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
  , Image
  , loadImage
  , setTextures
  , spawnModel
    -- * Events
  , Event
  , registerEvent
  , sendEvent
  , readEvents
    -- * The scene
  , Camera (..)
  , defaultCamera
  , Light (..)
  , defaultLight
  , setCamera
  , setLight
  , setAmbient
  , setWindowTitle
    -- * Physics
  , Shape (..)
  , Collider (..)
  , colliderOf
  , BodyKind (..)
  , Body (..)
  , bodyOf
  , setCollider
  , setBody
  , applyImpulse
  , setVelocity
  , getVelocity
  , RayHit (..)
  , raycast
  , Contact (..)
    -- * Logging
  , logInfo
  , logWarn
  , logError
    -- * Engine components
  , Transform (..)
  , lookingAt
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
foreign import ccall unsafe "voxl_hs_event_register"
  c_event_register :: Ptr () -> Ptr CChar -> CSize -> CSize -> IO Word32
foreign import ccall unsafe "voxl_hs_event_send" c_event_send :: Ptr () -> Word32 -> Ptr () -> IO ()
foreign import ccall unsafe "voxl_hs_event_next" c_event_next :: Ptr () -> Word32 -> Ptr () -> IO Word8
foreign import ccall unsafe "voxl_hs_set_camera"
  c_set_camera :: Ptr () -> Word64 -> CFloat -> CFloat -> Word32 -> IO ()
foreign import ccall unsafe "voxl_hs_set_light"
  c_set_light :: Ptr () -> Word64 -> CFloat -> CFloat -> CFloat -> CFloat -> Word32 -> IO ()
foreign import ccall unsafe "voxl_hs_set_ambient"
  c_set_ambient :: Ptr () -> CFloat -> CFloat -> CFloat -> CFloat -> IO ()
foreign import ccall unsafe "voxl_hs_set_window_title"
  c_set_window_title :: Ptr () -> Ptr CChar -> CSize -> IO ()
foreign import ccall unsafe "voxl_hs_set_collider"
  c_set_collider ::
    Ptr () -> Word64 -> Word32 -> CFloat -> CFloat -> CFloat -> CFloat -> CFloat -> Word32 -> IO ()
foreign import ccall unsafe "voxl_hs_set_body"
  c_set_body :: Ptr () -> Word64 -> Word32 -> CFloat -> CFloat -> CFloat -> CFloat -> Word32 -> IO ()
foreign import ccall unsafe "voxl_hs_apply_impulse"
  c_apply_impulse :: Ptr () -> Word64 -> CFloat -> CFloat -> CFloat -> IO ()
foreign import ccall unsafe "voxl_hs_set_velocity"
  c_set_velocity :: Ptr () -> Word64 -> CFloat -> CFloat -> CFloat -> IO ()
foreign import ccall unsafe "voxl_hs_velocity" c_velocity :: Ptr () -> Word64 -> Ptr CFloat -> IO Word8
foreign import ccall unsafe "voxl_hs_raycast"
  c_raycast ::
    Ptr () -> CFloat -> CFloat -> CFloat -> CFloat -> CFloat -> CFloat -> CFloat -> Ptr Word64 ->
    Ptr CFloat -> IO Word8
foreign import ccall unsafe "voxl_hs_component_describe"
  c_component_describe ::
    Ptr () -> Word32 -> Ptr CChar -> Ptr CSize -> Ptr Word32 -> Ptr Word32 -> Ptr CSize -> CSize ->
    IO CInt
foreign import ccall unsafe "voxl_hs_image_load" c_image_load :: Ptr () -> Ptr CChar -> CSize -> IO Word32
foreign import ccall unsafe "voxl_hs_set_textures"
  c_set_textures :: Ptr () -> Word64 -> Word32 -> Word32 -> Word32 -> IO ()
foreign import ccall unsafe "voxl_hs_spawn_model"
  c_spawn_model :: Ptr () -> Ptr CChar -> CSize -> Ptr Transform -> IO Word64
foreign import ccall unsafe "voxl_hs_set_parent" c_set_parent :: Ptr () -> Word64 -> Word64 -> IO ()
foreign import ccall unsafe "voxl_hs_despawn_tree" c_despawn_tree :: Ptr () -> Word64 -> IO ()
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

-- | What one field of a component holds.
data FieldType
  = FieldF32
  | FieldF64
  | FieldI32
  | FieldI64
  | FieldU8
  | FieldU32
  | -- | One byte; zero is false.
    FieldBool
  | -- | An 'Entity'. Scenes keep it pointing at the right entity.
    FieldEntity
  deriving (Eq, Show, Enum)

-- | One field of a component: its name, what it holds, how many in a row (more than one for
-- a vector or an array), and how many bytes into the component it starts.
data Field = Field String FieldType Int Int
  deriving (Eq, Show)

-- | Says what fields a component this plugin registered has, as its 'Storable' instance lays
-- them out. From then on the engine can save the component in scenes, load it back, and show
-- it in an inspector. Bytes that no field covers aren't saved, and load as zero.
describeComponent :: App -> Component a -> [Field] -> IO ()
describeComponent (App app) (Component handle) fields =
  withCStringLen (concat [name | Field name _ _ _ <- fields]) $ \(names, _) -> do
    -- The length of each name in bytes, which is what the engine counts in.
    lens <- mapM (\(Field name _ _ _) -> withCStringLen name (pure . fromIntegral . snd)) fields
    status <-
      withArrayLen (lens :: [CSize]) $ \count lensPtr ->
        withArrayLen [fromIntegral (fromEnum kind) | Field _ kind _ _ <- fields] $ \_ types ->
          withArrayLen [fromIntegral n | Field _ _ n _ <- fields] $ \_ counts ->
            withArrayLen [fromIntegral offset | Field _ _ _ offset <- fields] $ \_ offsets ->
              c_component_describe app handle names lensPtr types counts offsets (fromIntegral count)
    when (status /= 0) $
      ioError (userError "could not describe the component (see the engine's log)")

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

-- | Despawns an entity and everything below it (a model and its parts, say), when this
-- system returns.
despawnTree :: System -> Entity -> IO ()
despawnTree (System system) (Entity entity) = c_despawn_tree system entity

-- | Makes the first entity a child of the second, so that its transform is relative to the
-- parent's, or a root again with 'Nothing'. Takes effect when this system returns.
setParent :: System -> Entity -> Maybe Entity -> IO ()
setParent (System system) (Entity child) parent =
  c_set_parent system child (maybe maxBound (\(Entity bits) -> bits) parent)

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
  (,) . realToFrac <$> peekElemOff delta 0 <*> (realToFrac <$> peekElemOff delta 1)

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

-- | An event type whose events are values of @a@.
newtype Event a = Event Word32

-- | Defines an event type, or finds the one the name already has. Any plugin that knows the
-- name can send and read it, in any language; that is how plugins talk to each other. The
-- engine's own events are found the same way: @"voxl.Contact"@ is a 'Contact'.
registerEvent :: forall a. Storable a => App -> String -> IO (Event a)
registerEvent (App app) name = do
  handle <- withName name $ \chars len ->
    c_event_register app chars len (fromIntegral (sizeOf (undefined :: a)))
  when (handle == 0) $
    ioError (userError ("could not register event `" ++ name ++ "` (is it another size?)"))
  pure (Event handle)

-- | Sends an event when this system returns. Systems later this frame, and every system next
-- frame, can read it.
sendEvent :: Storable a => System -> Event a -> a -> IO ()
sendEvent (System system) (Event handle) value =
  Foreign.with value $ \ptr -> c_event_send system handle (castPtr ptr)

-- | The events this system hasn't read yet, oldest first. Each system reads each event once,
-- and a reloaded system carries on where the old one stopped.
readEvents :: Storable a => System -> Event a -> IO [a]
readEvents (System system) (Event handle) = alloca $ \ptr ->
  let loop seen = do
        more <- c_event_next system handle (castPtr ptr)
        if more /= 0 then peek ptr >>= \event -> loop (event : seen) else pure (reverse seen)
   in loop []

flag :: Bool -> Word32
flag = fromIntegral . fromEnum

-- | What the scene is seen through. The first active camera is the one drawn.
data Camera = Camera
  { fieldOfView :: !Float
    -- ^ vertical, in radians
  , nearPlane :: !Float
  , cameraActive :: !Bool
  }
  deriving (Eq, Show)

-- | A camera with a 60 degree field of view.
defaultCamera :: Camera
defaultCamera = Camera (pi / 3) 0.1 True

-- | A sun-like light, shining along the forward direction (-Z) of the entity's transform.
data Light = Light
  { lightColor :: !V3
  , lightIntensity :: !Float
  , castsShadows :: !Bool
  }
  deriving (Eq, Show)

-- | White sunlight that casts shadows.
defaultLight :: Light
defaultLight = Light (V3 1 1 1) 3 True

-- | Makes the entity a camera (it also needs a 'Transform'), when this system returns.
setCamera :: System -> Entity -> Camera -> IO ()
setCamera (System system) (Entity entity) (Camera fov near on) =
  c_set_camera system entity (realToFrac fov) (realToFrac near) (flag on)

-- | Makes the entity a light (it also needs a 'Transform'), when this system returns.
setLight :: System -> Entity -> Light -> IO ()
setLight (System system) (Entity entity) (Light (V3 r g b) strength cast) =
  c_set_light system entity (realToFrac r) (realToFrac g) (realToFrac b) (realToFrac strength) (flag cast)

-- | Sets the light arriving from every direction: a colour and a strength.
setAmbient :: System -> V3 -> Float -> IO ()
setAmbient (System system) (V3 r g b) strength =
  c_set_ambient system (realToFrac r) (realToFrac g) (realToFrac b) (realToFrac strength)

setWindowTitle :: System -> String -> IO ()
setWindowTitle (System system) title = withName title (c_set_window_title system)

-- | The solid form of a collider.
data Shape
  = Sphere !Float
  | -- | Half extents along x, y and z.
    Box !V3
  | -- | Upright: a radius and a height.
    Capsule !Float !Float
  | -- | Everything below the entity's position.
    Ground
  deriving (Eq, Show)

-- | Makes an entity solid. Without a 'Body' it never moves.
data Collider = Collider
  { colliderShape :: !Shape
  , friction :: !Float
  , restitution :: !Float
    -- ^ bounciness, 0 to 1
  , isSensor :: !Bool
    -- ^ detects overlaps but blocks nothing
  }
  deriving (Eq, Show)

-- | A collider of the given shape with ordinary friction and no bounce.
colliderOf :: Shape -> Collider
colliderOf form = Collider form 0.5 0 False

data BodyKind
  = -- | Moved by gravity, forces and collisions.
    Dynamic
  | -- | Moved only by its velocity; pushes other bodies and is never pushed.
    Kinematic
  | -- | Moved by setting its transform; pushes other bodies and is never pushed.
    Animated
  deriving (Eq, Show, Enum)

-- | Makes an entity with a collider move.
data Body = Body
  { bodyKind :: !BodyKind
  , bodyMass :: !Float
    -- ^ kilograms; 0 to work it out from the collider's size
  , bodyVelocity :: !V3
  , lockRotation :: !Bool
    -- ^ keeps it upright
  }
  deriving (Eq, Show)

-- | A body of the given kind, at rest, with its mass worked out from its collider.
bodyOf :: BodyKind -> Body
bodyOf how = Body how 0 (V3 0 0 0) False

-- | Makes the entity solid, when this system returns. Does nothing in an app without physics.
setCollider :: System -> Entity -> Collider -> IO ()
setCollider (System system) (Entity entity) (Collider form rough bounce ghost) =
  c_set_collider system entity code (realToFrac x) (realToFrac y) (realToFrac z)
    (realToFrac rough) (realToFrac bounce) (flag ghost)
  where
    (code, V3 x y z) = case form of
      Sphere radius -> (0, V3 radius 0 0)
      Box half -> (1, half)
      Capsule radius height -> (2, V3 radius height 0)
      Ground -> (3, V3 0 0 0)

-- | Makes an entity with a collider move, when this system returns.
setBody :: System -> Entity -> Body -> IO ()
setBody (System system) (Entity entity) (Body how kilograms (V3 x y z) upright) =
  c_set_body system entity (fromIntegral (fromEnum how)) (realToFrac kilograms)
    (realToFrac x) (realToFrac y) (realToFrac z) (flag upright)

-- | A sudden push on a dynamic body, in newton-seconds.
applyImpulse :: System -> Entity -> V3 -> IO ()
applyImpulse (System system) (Entity entity) (V3 x y z) =
  c_apply_impulse system entity (realToFrac x) (realToFrac y) (realToFrac z)

setVelocity :: System -> Entity -> V3 -> IO ()
setVelocity (System system) (Entity entity) (V3 x y z) =
  c_set_velocity system entity (realToFrac x) (realToFrac y) (realToFrac z)

peekV3 :: Ptr CFloat -> Int -> IO V3
peekV3 floats at =
  V3 . realToFrac <$> peekElemOff floats at
    <*> (realToFrac <$> peekElemOff floats (at + 1))
    <*> (realToFrac <$> peekElemOff floats (at + 2))

-- | The body's velocity, or 'Nothing' if the entity has no body.
getVelocity :: System -> Entity -> IO (Maybe V3)
getVelocity (System system) (Entity entity) = allocaArray 3 $ \floats -> do
  found <- c_velocity system entity floats
  if found /= 0 then Just <$> peekV3 floats 0 else pure Nothing

-- | What a ray met.
data RayHit = RayHit
  { hitEntity :: !Entity
  , hitPoint :: !V3
  , hitNormal :: !V3
  , hitDistance :: !Float
  }
  deriving (Eq, Show)

-- | The first collider a ray from the origin along the direction meets within the distance,
-- as things stood after the last physics step.
raycast :: System -> V3 -> V3 -> Float -> IO (Maybe RayHit)
raycast (System system) (V3 ox oy oz) (V3 dx dy dz) reach =
  alloca $ \entityOut -> allocaArray 7 $ \floats -> do
    found <-
      c_raycast system (realToFrac ox) (realToFrac oy) (realToFrac oz) (realToFrac dx)
        (realToFrac dy) (realToFrac dz) (realToFrac reach) entityOut floats
    if found == 0
      then pure Nothing
      else do
        entity <- peek entityOut
        hit <- RayHit (Entity entity) <$> peekV3 floats 0 <*> peekV3 floats 3
        Just . hit . realToFrac <$> peekElemOff floats 6

-- | The engine's @"voxl.Contact"@ event: two colliders touched during a physics step.
data Contact = Contact
  { contactA :: !Entity
  , contactB :: !Entity
  , contactPoint :: !V3
  , contactNormal :: !V3
    -- ^ from the first toward the second
  , contactImpulse :: !Float
    -- ^ how hard, in newton-seconds
  }
  deriving (Eq, Show)

instance Storable Contact where
  sizeOf _ = 48
  alignment _ = 8
  peek ptr =
    Contact . Entity <$> peek (castPtr ptr) <*> (Entity <$> peek (castPtr (ptr `plusPtr` 8)))
      <*> peek (castPtr (ptr `plusPtr` 16)) <*> peek (castPtr (ptr `plusPtr` 28))
      <*> peek (castPtr (ptr `plusPtr` 40))
  poke ptr (Contact (Entity a) (Entity b) point normal push) = do
    poke (castPtr ptr) a
    poke (castPtr (ptr `plusPtr` 8)) b
    poke (castPtr (ptr `plusPtr` 16)) point
    poke (castPtr (ptr `plusPtr` 28)) normal
    poke (castPtr (ptr `plusPtr` 40)) push

-- | An image, shared between the materials that use it. 'Storable', so it can be kept in a
-- 'statePtr' block.
newtype Image = Image Word32
  deriving (Eq, Show)

instance Storable Image where
  sizeOf _ = 4
  alignment _ = 4
  peek ptr = Image <$> peek (castPtr ptr)
  poke ptr (Image handle) = poke (castPtr ptr) handle

-- | Loads an image by name: a PNG or JPEG path relative to the app's asset folder, with
-- @?linear@ appended for data such as normal maps. The same name always gives the same image.
-- It arrives a moment later; until then it draws as plain white.
loadImage :: System -> String -> IO Image
loadImage (System system) name = do
  handle <- withName name (c_image_load system)
  when (handle == 0) $ ioError (userError ("could not load `" ++ name ++ "` (no asset server?)"))
  pure (Image handle)

-- | Sets the textures of the entity's material, when this system returns: its colour, its
-- normal map, and its roughness and metalness.
setTextures :: System -> Entity -> Maybe Image -> Maybe Image -> Maybe Image -> IO ()
setTextures (System system) (Entity entity) baseColor normal metallicRoughness =
  c_set_textures system entity (raw baseColor) (raw normal) (raw metallicRoughness)
  where
    raw = maybe 0 (\(Image handle) -> handle)

-- | Spawns a glTF model (@.gltf@ or @.glb@) by name: a new entity at the transform, with a
-- child for each part of the model. The parts appear when this system returns.
spawnModel :: System -> String -> Transform -> IO Entity
spawnModel (System system) name at =
  withName name $ \chars len ->
    Foreign.with at (fmap Entity . c_spawn_model system chars len)

-- What the engine calls for every Haskell system: `user` is the stable pointer to its
-- function. An exception must not escape into the engine, so it is logged instead.
foreign export ccall "voxl_hs_dispatch" dispatch :: Ptr () -> Ptr () -> IO ()

dispatch :: Ptr () -> Ptr () -> IO ()
dispatch system user = do
  run <- deRefStablePtr (castPtrToStablePtr user)
  result <- try (run system)
  case result of
    Right () -> pure ()
    Left (err :: SomeException) -> logError ("a system threw: " ++ displayException err)

foreign export ccall "voxl_hs_unload" unload :: IO ()

unload :: IO ()
unload = atomicModifyIORef' systems ([],) >>= mapM_ freeStablePtr

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

-- | A transform at the first point, turned so that its forward direction (-Z, the way a
-- camera sees and a light shines) points at the second. Not for looking straight up or down.
lookingAt :: V3 -> V3 -> Transform
lookingAt eye@(V3 ex ey ez) (V3 tx ty tz) = Transform eye orientation (V3 1 1 1)
  where
    unit (x, y, z) = let len = sqrt (x * x + y * y + z * z) in (x / len, y / len, z / len)
    cross (ax, ay, az) (bx, by, bz) = (ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx)
    forward@(fx, fy, fz) = unit (tx - ex, ty - ey, tz - ez)
    right@(rx, ry, rz) = unit (cross forward (0, 1, 0))
    (ux, uy, uz) = cross right forward
    -- The rotation whose columns are right, up and back, as a quaternion.
    (m00, m01, m02) = (rx, ux, -fx)
    (m10, m11, m12) = (ry, uy, -fy)
    (m20, m21, m22) = (rz, uz, -fz)
    trace = m00 + m11 + m22
    orientation
      | trace > 0 =
          let s = sqrt (trace + 1) * 2
           in Quat ((m21 - m12) / s) ((m02 - m20) / s) ((m10 - m01) / s) (s / 4)
      | m00 > m11 && m00 > m22 =
          let s = sqrt (1 + m00 - m11 - m22) * 2
           in Quat (s / 4) ((m01 + m10) / s) ((m02 + m20) / s) ((m21 - m12) / s)
      | m11 > m22 =
          let s = sqrt (1 + m11 - m00 - m22) * 2
           in Quat ((m01 + m10) / s) (s / 4) ((m12 + m21) / s) ((m02 - m20) / s)
      | otherwise =
          let s = sqrt (1 + m22 - m00 - m11) * 2
           in Quat ((m02 + m20) / s) ((m12 + m21) / s) (s / 4) ((m10 - m01) / s)

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
