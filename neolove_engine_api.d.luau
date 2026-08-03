-- neolove engine api definitions

export type Color4Value = {
	r: number,
	g: number,
	b: number,
	a: number,
}

export type Vec2 = {
	x: number,
	y: number,
}

export type Vec3 = {
	x: number,
	y: number,
	z: number,
}

export type PositionPivot = "center" | "top_right"

export type EntityListenEvent = "leftClick" | "rightClick" | "middleClick" | "scrollUp" | "scrollDown" | "mouseEntered" | "mouseExited"

export type EntityListenInfo = {
	kind: EntityListenEvent,
	type: EntityListenEvent,
	button: "left" | "right" | "middle"?,
	x: number,
	y: number,
	mouseX: number,
	mouseY: number,
	localX: number,
	localY: number,
	local_x: number,
	local_y: number,
	wheelX: number,
	wheelY: number,
	amount: number,
}

export type Connection = {
	Disconnect: (self: Connection) -> boolean,
	disconnect: (self: Connection) -> boolean,
	IsConnected: (self: Connection) -> boolean,
	isConnected: (self: Connection) -> boolean,
}

export type Entity = {
	id: number,
	name: string,
	x: number,
	y: number,
	-- 3D position. The X/Y axes continue to use x and y.
	position_z: number,
	anchor_x: number,
	anchor_y: number,
	pivot_x: number?,
	pivot_y: number?,
	rotation: number,
	-- XYZ Euler angles in degrees for 3D transforms.
	rotation_x: number,
	rotation_y: number,
	rotation_z: number,
	rotation_pivot: string,
	rotation_pivot_x: number?,
	rotation_pivot_y: number?,
	position_pivot: PositionPivot?,
	z: number,
	size_x: number,
	size_y: number,
	scale: number,
	scale_x: number,
	scale_y: number,
	scale_z: number,
	raycastable: boolean?,
	parent: Entity?,
	children: { Entity },
	components: { ComponentInstance },
	listen: (self: Entity, event: EntityListenEvent | string, callback: (entity: Entity, event: EntityListenInfo) -> ()) -> Connection,
	Listen: (self: Entity, event: EntityListenEvent | string, callback: (entity: Entity, event: EntityListenInfo) -> ()) -> Connection,
	delete: (self: Entity) -> (),
	Delete: (self: Entity) -> (),
	addComponent: (self: Entity, component: Component) -> ComponentInstance,
	AddComponent: (self: Entity, component: Component) -> ComponentInstance,
	removeComponent: (self: Entity, target: number | ComponentInstance) -> boolean,
	RemoveComponent: (self: Entity, target: number | ComponentInstance) -> boolean,
	duplicate: (self: Entity, parent: Entity?) -> Entity,
	Duplicate: (self: Entity, parent: Entity?) -> Entity,
	findFirstChild: (self: Entity, name: string) -> Entity?,
	FindFirstChild: (self: Entity, name: string) -> Entity?,
	getWorldPosition: (self: Entity) -> (number, number),
	GetWorldPosition: (self: Entity) -> (number, number),
	getWorldRotation: (self: Entity) -> number,
	GetWorldRotation: (self: Entity) -> number,
	getWorldPosition3D: (self: Entity) -> (number, number, number),
	GetWorldPosition3D: (self: Entity) -> (number, number, number),
	getWorldRotation3D: (self: Entity) -> (number, number, number),
	GetWorldRotation3D: (self: Entity) -> (number, number, number),
	isInside: (self: Entity, world_x: number, world_y: number) -> boolean,
	IsInside: (self: Entity, world_x: number, world_y: number) -> boolean,
	[string]: any,
}

export type System = {
	awake: ((self: System) -> ())?,
	update: ((self: System, dt: number) -> ())?,
	lateUpdate: ((self: System, dt: number) -> ())?,
	fixedUpdate: ((self: System, dt: number) -> ())?,
	[string]: any,
}

export type CollisionCallback = (
	selfEntity: Entity,
	selfCollider: Collider2D,
	otherEntity: Entity,
	otherCollider: Collider2D,
	otherId: number
) -> ()

export type Component = {
	name: string?,
	__neolove_component: string?,
	awake: ((entity: Entity, component: ComponentInstance) -> ())?,
	update: ((entity: Entity, component: ComponentInstance, dt: number) -> ())?,
	destroy: ((entity: Entity, component: ComponentInstance) -> ())?,
	onDestroy: ((entity: Entity, component: ComponentInstance) -> ())?,
	NEOLOVE_RENDERING: boolean?,
	[string]: any,
}

export type ComponentInstance = Component & {
	entity: Entity?,
	remove: (self: ComponentInstance) -> boolean,
	Remove: (self: ComponentInstance) -> boolean,
	getEntity: (self: ComponentInstance) -> Entity?,
	GetEntity: (self: ComponentInstance) -> Entity?,
	[string]: any,
}

export type ShaderHandle = {
	setUniform1f: (self: ShaderHandle, name: string, x: number) -> (),
	setUniform2f: (self: ShaderHandle, name: string, x: number, y: number) -> (),
	setUniform3f: (self: ShaderHandle, name: string, x: number, y: number, z: number) -> (),
	setUniform4f: (self: ShaderHandle, name: string, x: number, y: number, z: number, w: number) -> (),
	setUniformColor: (self: ShaderHandle, name: string, color: Color4Value) -> (),
	setTexture: (self: ShaderHandle, name: string, image: ImageHandle) -> (),
}

export type ImageHandle = {
	width: (self: ImageHandle) -> number,
	height: (self: ImageHandle) -> number,
	size: (self: ImageHandle) -> (number, number),
	getPixel: (self: ImageHandle, x: number, y: number) -> Color4Value,
	setPixel: (self: ImageHandle, x: number, y: number, color: Color4Value) -> (),
	fill: (self: ImageHandle, color: Color4Value) -> (),
	upload: (self: ImageHandle) -> (),
	export: (self: ImageHandle, path: string) -> (),
	save: (self: ImageHandle, path: string) -> (),
	unload: (self: ImageHandle) -> (),
	isUnloaded: (self: ImageHandle) -> boolean,
}

-- Vertex and index arguments use Lua's one-based indexing. A vertex accepts
-- flat x/y/z, u/v fields or nested position/normal/uv/tangent tables.
export type MeshVertexInput = {
	x: number?,
	y: number?,
	z: number?,
	u: number?,
	v: number?,
	position: Vec3?,
	normal: Vec3?,
	uv: ({ u: number, v: number } | Vec2)?,
	tangent: ({ x: number, y: number, z: number, w: number })?,
}

export type MeshVertex = {
	x: number,
	y: number,
	z: number,
	nx: number,
	ny: number,
	nz: number,
	u: number,
	v: number,
	tx: number,
	ty: number,
	tz: number,
	tw: number,
}

export type MeshBounds = {
	min_x: number,
	min_y: number,
	min_z: number,
	max_x: number,
	max_y: number,
	max_z: number,
	center_x: number,
	center_y: number,
	center_z: number,
	radius: number,
}

export type MeshMaterial = {
	name: string,
	metallic: number,
	roughness: number,
	double_sided: boolean,
	base_color_texture: string?,
	normal_texture: string?,
	base_color: Color4Value,
}

export type MeshMaterialTextureSlot =
	"base"
	| "base_color"
	| "albedo"
	| "diffuse"
	| "normal"
	| "normal_map"
	| "metallic_roughness"
	| "orm"
	| "emissive"
	| "emission"

export type PrimitiveMeshKind = "cube" | "sphere" | "plane" | "cylinder" | "capsule" | "cone"

export type PrimitiveMeshOptions = {
	-- `size` sets all three axes; axis-specific fields override it.
	size: number?,
	width: number?,
	height: number?,
	depth: number?,
	length: number?,
	size_x: number?,
	size_y: number?,
	size_z: number?,
	radius: number?,
	segments: number?,
	rings: number?,
	segments_x: number?,
	segments_z: number?,
}

-- Mesh handles have shared identity. Successful edits commit atomically,
-- increment revision(), and are visible to renderers and mesh colliders.
export type MeshHandle = {
	revision: (self: MeshHandle) -> number,
	identity: (self: MeshHandle) -> number,
	vertexCount: (self: MeshHandle) -> number,
	indexCount: (self: MeshHandle) -> number,
	triangleCount: (self: MeshHandle) -> number,
	bounds: (self: MeshHandle) -> MeshBounds,
	getVertex: (self: MeshHandle, index: number) -> MeshVertex,
	setVertex: (self: MeshHandle, index: number, vertex: MeshVertexInput, recomputeNormals: boolean?) -> number,
	setPosition: (self: MeshHandle, index: number, x: number, y: number, z: number, recomputeNormals: boolean?) -> number,
	getIndex: (self: MeshHandle, index: number) -> number,
	setIndex: (self: MeshHandle, index: number, vertexIndex: number) -> number,
	replaceGeometry: (self: MeshHandle, vertices: { MeshVertexInput }, indices: { number }?) -> number,
	recomputeNormals: (self: MeshHandle) -> number,
	materialCount: (self: MeshHandle) -> number,
	getMaterial: (self: MeshHandle, index: number) -> MeshMaterial,
	setMaterialTexture: (self: MeshHandle, index: number, slot: MeshMaterialTextureSlot, source: string?, texCoord: number?) -> number,
	setMaterialColor: (self: MeshHandle, index: number, color: Color4Value) -> number,
	-- Cached imported/primitive handles share geometry and pose. Clone before
	-- independently animating several instances of the same asset.
	cloneDetached: (self: MeshHandle) -> MeshHandle,
	animationNames: (self: MeshHandle) -> { string },
	animationDuration: (self: MeshHandle, name: string) -> number?,
	sampleAnimation: (self: MeshHandle, name: string, time: number, looped: boolean?) -> number,
	playAnimation: (self: MeshHandle, name: string, looped: boolean?, speed: number?) -> (),
	updateAnimation: (self: MeshHandle, deltaSeconds: number) -> boolean,
	pauseAnimation: (self: MeshHandle, paused: boolean?) -> (),
	stopAnimation: (self: MeshHandle) -> (),
	jointCount: (self: MeshHandle) -> number,
}

export type SoundHandle = {
	sampleRate: (self: SoundHandle) -> number,
	channels: (self: SoundHandle) -> number,
	len: (self: SoundHandle) -> number,
	getSample: (self: SoundHandle, index: number) -> number,
	setSample: (self: SoundHandle, index: number, value: number) -> (),
	upload: (self: SoundHandle) -> (),
	export: (self: SoundHandle, path: string) -> (),
	save: (self: SoundHandle, path: string) -> (),
	unload: (self: SoundHandle) -> (),
	isUnloaded: (self: SoundHandle) -> boolean,
}

export type RaycastHit = {
	entity: Entity,
	id: number,
	distance: number,
	x: number,
	y: number,
	normalX: number,
	normalY: number,
	normal_x: number,
	normal_y: number,
}

export type RaycastOptions = {
	ignore: Entity | { Entity }?,
	ignoreEntity: Entity | { Entity }?,
}

export type AppModule = {
	bg: Color4Value,
	antiAliasing: "off" | "standard" | "high",
	setMaxFps: (fps: number?) -> (),
	getMaxFps: () -> number?,
	setShowFps: (enabled: boolean?) -> (),
	getShowFps: () -> boolean,
	nearestNeighborScaling: boolean,
	setNearestNeighborScaling: (enabled: boolean?) -> (),
	getNearestNeighborScaling: () -> boolean,
	setAntiAliasing: (mode: ("off" | "standard" | "high")?) -> (),
	getAntiAliasing: () -> "off" | "standard" | "high",
}

export type InputModule = {
	isKeyDown: (key: string) -> boolean,
	isKeyPressed: (key: string) -> boolean,
	isKeyReleased: (key: string) -> boolean,
	isMouseDown: (button: string?) -> boolean,
	isMousePressed: (button: string?) -> boolean,
	isMouseReleased: (button: string?) -> boolean,
	getMouseWheel: () -> (number, number),
	isScrollingIn: () -> boolean,
	isScrollingOut: () -> boolean,
	getScrollInAmount: () -> number,
	getMouseDelta: () -> (number, number),
	setMouseLocked: (locked: boolean) -> (),
	isMouseLocked: () -> boolean,
	getLastKeyPressed: () -> string?,
	getCharPressed: () -> string?,
	showKeyboard: (implicit: boolean?) -> boolean,
	openKeyboard: (implicit: boolean?) -> boolean,
	hideKeyboard: (implicitOnly: boolean?) -> boolean,
	closeKeyboard: (implicitOnly: boolean?) -> boolean,
}

export type AssetsModule = {
	loadImage: (pathOrBase64Png: string) -> ImageHandle,
	loadImageBase64: (base64Png: string) -> ImageHandle,
	-- Imports .obj, .gltf, .glb, or ASCII/binary .fbx geometry. glTF/GLB and
	-- supported ASCII FBX files may also contain one armature and animation clips.
	loadMesh: (path: string) -> MeshHandle,
	primitiveMesh: (kind: PrimitiveMeshKind | string, options: PrimitiveMeshOptions?) -> MeshHandle,
	newMesh: (vertices: { MeshVertexInput }, indices: { number }?) -> MeshHandle,
	snapPhoto: (x: number, y: number, x2: number, y2: number) -> ImageHandle,
	newImage: (width: number, height: number, color: Color4Value?) -> ImageHandle,
	loadSound: (path: string) -> SoundHandle,
	newSound: (sampleRate: number, channels: number, len: number, fill: number?) -> SoundHandle,
	unloadImage: (value: string | ImageHandle) -> boolean,
	unloadSound: (value: string | SoundHandle) -> boolean,
	-- Evicts a path from the cache; existing handles remain valid.
	unloadMesh: (path: string) -> boolean,
	gc: () -> (number, number),
}

export type AudioModule = {
	play: (sound: SoundHandle, looped: boolean?, volume: number?) -> (),
	playOnce: (sound: SoundHandle, volume: number?) -> (),
	stop: (sound: SoundHandle) -> (),
	setVolume: (sound: SoundHandle, volume: number) -> (),
	playSpatial: (sound: SoundHandle, x: number, y: number, looped: boolean?, volume: number?) -> (),
	setPosition: (sound: SoundHandle, x: number, y: number) -> boolean,
	setListenerPosition: (x: number, y: number) -> (),
}

export type MediaPermissionStatus = "prompt" | "granted" | "denied" | "unavailable"
export type MediaDeviceKind = "microphone" | "camera"
export type MediaEnumerationKind = MediaDeviceKind | "all" | "both"

export type MediaDevice = {
	id: string,
	kind: MediaDeviceKind,
	label: string,
	isDefault: boolean,
}

export type MediaAudioConstraints = {
	deviceId: string?,
	sampleRate: number?,
	channels: number?,
	echoCancellation: boolean?,
	noiseSuppression: boolean?,
	autoGainControl: boolean?,
}

export type MediaVideoConstraints = {
	deviceId: string?,
	width: number?,
	height: number?,
	frameRate: number?,
	facingMode: ("user" | "environment" | "left" | "right")?,
}

export type MediaRequestOptions = {
	audio: (boolean | MediaAudioConstraints)?,
	video: (boolean | MediaVideoConstraints)?,
	microphone: (boolean | MediaAudioConstraints)?,
	camera: (boolean | MediaVideoConstraints)?,
}

export type MediaAudioFormat = {
	sampleRate: number,
	channels: number,
}

export type MediaVideoFormat = {
	width: number,
	height: number,
	frameRate: number,
}

export type MediaAudioSamples = MediaAudioFormat & {
	frameCount: number,
	droppedSamples: number,
	format: "f32le",
	samples: { number },
}

export type MediaAudioBytes = MediaAudioFormat & {
	frameCount: number,
	droppedSamples: number,
	format: "f32le",
	data: string,
}

export type MediaVideoFrame = {
	-- This is a live image reused by its stream. A later readVideoFrame call
	-- updates the pixels of previously held frame images instead of snapshotting.
	image: ImageHandle,
	width: number,
	height: number,
	timestamp: number,
	droppedFrames: number,
}

export type MediaStream = {
	stop: (self: MediaStream) -> (),
	isActive: (self: MediaStream) -> boolean,
	hasAudio: (self: MediaStream) -> boolean,
	hasVideo: (self: MediaStream) -> boolean,
	getAudioFormat: (self: MediaStream) -> MediaAudioFormat?,
	getVideoFormat: (self: MediaStream) -> MediaVideoFormat?,
	readAudio: (self: MediaStream, maxFrames: number?) -> MediaAudioSamples?,
	readAudioBytes: (self: MediaStream, maxFrames: number?) -> MediaAudioBytes?,
	readVideoFrame: (self: MediaStream) -> MediaVideoFrame?,
	getLastError: (self: MediaStream) -> string?,
}

export type MediaDeviceResult = {
	ok: boolean,
	devices: { MediaDevice }?,
	code: string?,
	error: string?,
}

export type MediaAccessResult = {
	ok: boolean,
	stream: MediaStream?,
	code: string?,
	error: string?,
}

export type MicrophoneModule = {
	listDevices: (callback: (result: MediaDeviceResult) -> ()) -> number,
	enumerateDevices: (callback: (result: MediaDeviceResult) -> ()) -> number,
	requestAccess: (((result: MediaAccessResult) -> ()) -> number)
		& ((constraints: MediaAudioConstraints, callback: (result: MediaAccessResult) -> ()) -> number)
		& ((deviceId: string, callback: (result: MediaAccessResult) -> ()) -> number),
	requestDevice: (deviceId: string, callback: (result: MediaAccessResult) -> ()) -> number,
	cancelRequest: (requestId: number) -> boolean,
	getPermissionStatus: () -> MediaPermissionStatus,
	isSupported: () -> boolean,
}

export type MediaModule = {
	enumerateDevices: (((result: MediaDeviceResult) -> ()) -> number) & ((kind: MediaEnumerationKind, callback: (result: MediaDeviceResult) -> ()) -> number),
	requestAccess: (options: MediaRequestOptions, callback: (result: MediaAccessResult) -> ()) -> number,
	cancelRequest: (requestId: number) -> boolean,
	getPermissionStatus: (kind: MediaDeviceKind) -> MediaPermissionStatus,
	permissions: () -> { microphone: MediaPermissionStatus, camera: MediaPermissionStatus },
	-- Reports platform/backend capability, not whether hardware is currently attached.
	-- "all"/"both" is true only when microphone and camera capture are both supported.
	isSupported: (kind: MediaEnumerationKind) -> boolean,
	stopAll: () -> number,
	_poll: () -> (),
	microphone: MicrophoneModule,
}

export type FsWalkEntry = {
	path: string,
	name: string,
	kind: "file" | "directory",
	isFile: boolean,
	isDir: boolean,
	is_file: boolean,
	is_dir: boolean,
}

export type FsModule = {
	isWebasm: () -> boolean,
	isWebAssembly: () -> boolean,
	isMobile: () -> boolean,
	isAndroid: () -> boolean,
	openFilePicker: () -> string?,
	openFolderPicker: () -> string?,
	getDataDirectory: () -> string,
	dataPath: (path: string) -> string,
	readFile: (path: string) -> string,
	readBytes: (path: string) -> string,
	writeFile: (path: string, content: string) -> (),
	appendFile: (path: string, content: string) -> (),
	exists: (path: string) -> boolean,
	isFile: (path: string) -> boolean,
	isDir: (path: string) -> boolean,
	createDir: (path: string) -> (),
	walk: (path: string?, recursive: boolean?) -> { FsWalkEntry },
	rename: (from: string, to: string) -> (),
	copy: (from: string, to: string) -> (),
	removeFile: (path: string) -> boolean,
}

export type AndroidModule = {
	isAndroid: () -> boolean,
	getDeviceId: () -> string?,
	getSdkInt: () -> number?,
	getApiLevel: () -> number?,
	getBrand: () -> string?,
	getManufacturer: () -> string?,
	getModel: () -> string?,
	getDevice: () -> string?,
	getProduct: () -> string?,
	showKeyboard: (implicit: boolean?) -> boolean,
	openKeyboard: (implicit: boolean?) -> boolean,
	hideKeyboard: (implicitOnly: boolean?) -> boolean,
	closeKeyboard: (implicitOnly: boolean?) -> boolean,
}

export type MobileModule = {
	isMobile: () -> boolean,
	isEmulated: () -> boolean,
	isOnline: () -> boolean,
	isWifiEnabled: () -> boolean,
	isCellularEnabled: () -> boolean,
	isLowPowerMode: () -> boolean,
	getNetworkType: () -> "wifi" | "cellular" | "offline",
	getOrientation: () -> "portrait" | "landscape",
	isLandscape: () -> boolean,
	getDeviceSize: () -> (number, number),
	getSafeAreaInsets: () -> (number, number, number, number),
}

export type AsyncTask = {
	id: number,
	done: boolean,
	cancelled: boolean,
	status: "queued" | "running" | "suspended" | "completed" | "cancelled" | "error",
	error: string?,
	result: any,
	results: { any },
	cancel: (self: AsyncTask) -> boolean,
	Cancel: (self: AsyncTask) -> boolean,
	isDone: (self: AsyncTask) -> boolean,
	IsDone: (self: AsyncTask) -> boolean,
	getStatus: (self: AsyncTask) -> string,
	GetStatus: (self: AsyncTask) -> string,
	getError: (self: AsyncTask) -> string?,
	GetError: (self: AsyncTask) -> string?,
	getResult: (self: AsyncTask) -> ...any,
	GetResult: (self: AsyncTask) -> ...any,
}

export type AsyncModule = {
	yield: (...any) -> ...any,
	count: () -> number,
	cancelAll: () -> number,
} & ((callback: () -> ...any) -> AsyncTask)

export type HttpHeaders = { [string]: string }

export type HttpResponse = {
	ok: boolean,
	url: string,
	status: number?,
	body: string,
	error: string?,
	headers: HttpHeaders,
}

export type HttpRequestOptions = {
	url: string,
	method: string?,
	headers: HttpHeaders?,
	body: string?,
}

export type HttpModule = {
	request: ((url: string, callback: (response: HttpResponse) -> ()) -> number) & ((options: HttpRequestOptions, callback: (response: HttpResponse) -> ()) -> number),
	get: (url: string, callback: (response: HttpResponse) -> ()) -> number,
	_poll: () -> (),
}

export type CommandRunResult = {
	ok: boolean,
	statusCode: number,
	status_code: number,
	stdout: string,
	stderr: string,
	error: string?,
}

export type CommandDetachedResult = {
	ok: boolean,
	pid: number,
	error: string?,
}

export type CommandsModule = {
	run: (command: string, args: { string }?, cwd: string?) -> CommandRunResult,
	runDetached: (command: string, args: { string }?, cwd: string?) -> CommandDetachedResult,
}

export type ServerHostOptions = {
	host: string?,
	certPath: string?,
	keyPath: string?,
	cert_path: string?,
	key_path: string?,
}

export type ServerClientHandle = {
	key: string,
	is_host: boolean,
	send: (payload: buffer) -> boolean,
	addCallback: (callback: (payload: buffer) -> ()) -> (),
	addcallback: (callback: (payload: buffer) -> ()) -> (),
	disconnect: () -> boolean,
	isConnected: () -> boolean,
	getKey: () -> string,
	isHost: () -> boolean,
	getKickReason: () -> string?,
	on: ((self: ServerClientHandle, eventName: string, callback: (data: any, eventName: string, client: ServerClientHandle) -> ()) -> ((data: any, eventName: string, client: ServerClientHandle) -> ()))?,
	once: ((self: ServerClientHandle, eventName: string, callback: (data: any, eventName: string, client: ServerClientHandle) -> ()) -> ((data: any, eventName: string, client: ServerClientHandle) -> ()))?,
	off: ((self: ServerClientHandle, eventName: string, callback: (...any) -> ()) -> boolean)?,
	onAny: ((self: ServerClientHandle, callback: (eventName: string, data: any, client: ServerClientHandle) -> ()) -> ((eventName: string, data: any, client: ServerClientHandle) -> ()))?,
	emit: ((self: ServerClientHandle, eventName: string, data: any) -> boolean)?,
}

export type HostedServerHandle = {
	client: ServerClientHandle,
	port: number,
	url: string,
	stop: () -> boolean,
	getPort: () -> number,
	getUrl: () -> string,
	send: (self: HostedServerHandle, clientKey: string, payload: buffer) -> boolean,
	broadcast: (self: HostedServerHandle, payload: buffer) -> number,
	getClients: (self: HostedServerHandle) -> { string },
	getClientCount: (self: HostedServerHandle) -> number,
	emit: ((self: HostedServerHandle, eventName: string, data: any) -> number)?,
	sendEvent: ((self: HostedServerHandle, clientKey: string, eventName: string, data: any) -> boolean)?,
}

export type ServerPeer = {
	key: string,
	is_host: boolean,
	tags: { string },
	send: (self: ServerPeer, payload: buffer) -> boolean,
	emit: (self: ServerPeer, eventName: string, data: any) -> boolean,
	kick: (self: ServerPeer, reason: string?) -> (),
	isConnected: (self: ServerPeer) -> boolean,
}

export type ServerService = {
	name: string?,
	onStart: ((self: ServerService, host: HostedServerHandle) -> ())?,
	onConnect: ((self: ServerService, client: ServerPeer) -> ())?,
	onMessage: ((self: ServerService, client: ServerPeer, eventName: string, data: any) -> ())?,
	onDisconnect: ((self: ServerService, client: ServerPeer) -> ())?,
	host: (self: ServerService, port: number, options: ServerHostOptions?) -> HostedServerHandle,
	connect: (self: ServerService, url: string) -> ServerClientHandle,
	[string]: any,
}

export type ServersModule = {
	host: (scriptPath: string, port: number, options: ServerHostOptions?) -> HostedServerHandle,
	connect: (url: string) -> ServerClientHandle,
	define: (definition: { [string]: any }) -> ServerService,
	service: (definition: { [string]: any }) -> ServerService,
	createService: (definition: { [string]: any }) -> ServerService,
	create_service: (definition: { [string]: any }) -> ServerService,
	serializeTable: (value: any) -> buffer,
	serialize_table: (value: any) -> buffer,
	deserializeTable: (payload: buffer) -> any,
	deserialize_table: (payload: buffer) -> any,
	generateUuid4: () -> string,
	generate_uuid4: () -> string,
	generateUuid7: () -> string,
	generate_uuid7: () -> string,
	sha256: (value: string | buffer) -> string,
	sha128: (value: string | buffer) -> string,
	_poll: () -> (),
}

export type ShaderLoadOptions = {
	uniforms: { string }?,
	images: { string }?,
	textures: { string }?,
	pipelines: { string }?,
	[string]: any,
}

export type ShadersModule = {
	DEFAULT_VERTEX_SHADER: string,
	load: (vertexPath: string, fragmentPath: string, options: ShaderLoadOptions?) -> ShaderHandle,
	loadFragment: (fragmentPath: string, options: ShaderLoadOptions?) -> ShaderHandle,
	load3DFragment: (fragmentPath: string, options: ShaderLoadOptions?) -> ShaderHandle,
	fromSource: (vertexSource: string, fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
	fromFragmentSource: (fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
	from3DFragmentSource: (fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
	supports3D: () -> boolean,
	supports3DShaders: () -> boolean,
}

export type TransformModule = {
	getWorldPosition: (entity: Entity) -> (number, number),
	getWorldRotation: (entity: Entity) -> number,
	getWorldPosition3D: (entity: Entity) -> (number, number, number),
	get_world_position_3d: (entity: Entity) -> (number, number, number),
	getWorldRotation3D: (entity: Entity) -> (number, number, number),
	get_world_rotation_3d: (entity: Entity) -> (number, number, number),
	lookAt: (from_x: number, from_y: number, to_x: number, to_y: number) -> number,
	look_at: (from_x: number, from_y: number, to_x: number, to_y: number) -> number,
	GetEntitiesInFront: (world_x: number, world_y: number, minimum_z: number?) -> { Entity },
	getEntitiesInFront: (world_x: number, world_y: number, minimum_z: number?) -> { Entity },
	doTheyOverlap: (entities: { Entity }) -> boolean,
	raycast: (
		origin_x: number,
		origin_y: number,
		dir_x: number,
		dir_y: number,
		max_distance: number?,
		options: RaycastOptions?
	) -> RaycastHit?,
}

export type EcsModule = {
	addSystem: (system: System) -> (),
	newEntity: (name: string, parent: Entity?, x: number?, y: number?) -> Entity,
	deleteEntity: (entity: Entity) -> (),
	duplicateEntity: (targetEntity: Entity, parent: Entity) -> Entity,
	findFirstChild: (parent: Entity, name: string) -> Entity?,
	root: Entity,
	addComponent: (entity: Entity, component: Component) -> ComponentInstance,
	removeComponent: (entity: Entity, target: number | ComponentInstance) -> boolean,
	loadScene: (path: string) -> (),
}

export type PrefabTemplate = {
	name: string?,
	x: number?,
	y: number?,
	position_z: number?,
	anchor_x: number?,
	anchor_y: number?,
	pivot_x: number?,
	pivot_y: number?,
	rotation: number?,
	rotation_x: number?,
	rotation_y: number?,
	rotation_z: number?,
	rotation_pivot: string?,
	rotation_pivot_x: number?,
	rotation_pivot_y: number?,
	position_pivot: PositionPivot?,
	z: number?,
	size_x: number?,
	size_y: number?,
	scale: number?,
	scale_x: number?,
	scale_y: number?,
	scale_z: number?,
	parent: PrefabTemplate?,
	children: { PrefabTemplate }?,
	components: { Component }?,
	[string]: any,
}

export type PrefabUiModule = {
	label: PrefabTemplate,
	panel: PrefabTemplate,
	dialog: PrefabTemplate,
	statusChip: PrefabTemplate,
	status_chip: PrefabTemplate,
}

export type PrefabsModule = {
	capture: (entity: Entity) -> PrefabTemplate,
	component: <T>(source: T & Component, overrides: { [string]: any }?) -> T & Component,
	load: (path: string) -> PrefabTemplate,
	register: (name: string, source: string | Entity | PrefabTemplate) -> PrefabTemplate,
	get: (name: string) -> PrefabTemplate?,
	remove: (name: string) -> boolean,
	instantiate: (source: string | Entity | PrefabTemplate, parent: Entity?) -> Entity,
	duplicate: (source: string | Entity | PrefabTemplate, parent: Entity?) -> Entity,
	ui: PrefabUiModule,
}

export type EasingStyle =
	"linear"
	| "sine"
	| "quad"
	| "cubic"
	| "quart"
	| "quint"
	| "expo"
	| "circ"
	| "back"
	| "bounce"

export type EasingDirection = "in" | "out" | "inOut" | "in_out"

export type TweenHandle = {
	id: number,
	cancel: (self: TweenHandle) -> boolean,
	Cancel: (self: TweenHandle) -> boolean,
	isDone: (self: TweenHandle) -> boolean,
	IsDone: (self: TweenHandle) -> boolean,
}

export type TweeningModule = {
	to: (
		target: { [any]: any },
		key: any,
		value: number,
		duration: number,
		style: EasingStyle?,
		direction: EasingDirection?,
		onComplete: (() -> ())?
	) -> TweenHandle,
	new: (
		target: { [any]: any },
		key: any,
		value: number,
		duration: number,
		style: EasingStyle?,
		direction: EasingDirection?,
		onComplete: (() -> ())?
	) -> TweenHandle,
	create: (
		target: { [any]: any },
		key: any,
		value: number,
		duration: number,
		style: EasingStyle?,
		direction: EasingDirection?,
		onComplete: (() -> ())?
	) -> TweenHandle,
	cancelAll: () -> number,
	cancel_all: () -> number,
	count: () -> number,
	ease: (t: number, style: EasingStyle?, direction: EasingDirection?) -> number,
	update: (dt: number) -> (),
}

export type AnimationKeyframe = {
	time: number,
	value: number,
	out_x: number?,
	out_y: number?,
	in_x: number?,
	in_y: number?,
}
export type AnimationTrack = {
	property: string,
	interpolation: "linear" | "step" | "hold" | "bezier"?,
	keys: { AnimationKeyframe },
}
export type AnimationClip = {
	duration: number?,
	looping: boolean?,
	looped: boolean?,
	tracks: { AnimationTrack },
}
export type AnimationHandle = {
	id: number,
	play: (self: AnimationHandle) -> (),
	pause: (self: AnimationHandle) -> (),
	stop: (self: AnimationHandle) -> (),
	seek: (self: AnimationHandle, time: number) -> (),
	setSpeed: (self: AnimationHandle, speed: number) -> (),
	isPlaying: (self: AnimationHandle) -> boolean,
}
export type AnimationModule = {
	load: (path: string) -> AnimationClip,
	Load: (path: string) -> AnimationClip,
	new: (target: { [any]: any }, clip: AnimationClip) -> AnimationHandle,
	create: (target: { [any]: any }, clip: AnimationClip) -> AnimationHandle,
	play: (target: { [any]: any }, clip: AnimationClip) -> AnimationHandle,
}

export type BaseDrawableComponent = ComponentInstance & {
	NEOLOVE_RENDERING: boolean,
	color: Color4Value,
	shader: ShaderHandle?,
	visible: boolean,
}

export type Rect2D = BaseDrawableComponent

export type EntityScaler = ComponentInstance & {
	__neolove_component: "EntityScaler",
	enabled: boolean,
	edit_with_percent: boolean,
	editWithPercent: boolean?,
	x_percent: number,
	y_percent: number,
	size_x_percent: number,
	size_y_percent: number,
	xPercent: number?,
	yPercent: number?,
	sizeXPercent: number?,
	sizeYPercent: number?,
	percent_x: number?,
	percent_y: number?,
	percentX: number?,
	percentY: number?,
	offset_x: number,
	offset_y: number,
	offsetX: number?,
	offsetY: number?,
	pivot_x: number,
	pivot_y: number,
	pivotX: number?,
	pivotY: number?,
}

-- Centers the rendered scene on its entity. The first enabled Camera is used
-- automatically; call SetActive when a scene has more than one camera.
export type Camera = ComponentInstance & {
	__neolove_component: "Camera",
	enabled: boolean,
	-- Returns false only when the component is disabled or detached.
	SetActive: (self: Camera) -> boolean,
	setActive: (self: Camera) -> boolean,
	IsActive: (self: Camera) -> boolean,
	isActive: (self: Camera) -> boolean,
}

-- Camera transforms use entity x/y/position_z and XYZ Euler degrees. The
-- first enabled Camera3D is selected automatically unless SetActive is used.
export type Camera3D = ComponentInstance & {
	__neolove_component: "Camera3D",
	NEOLOVE_RENDERING: boolean,
	enabled: boolean,
	projection: "perspective" | "orthographic",
	fov: number,
	orthographic_size: number,
	near_clip: number,
	far_clip: number,
	SetActive: (self: Camera3D) -> boolean,
	setActive: (self: Camera3D) -> boolean,
	IsActive: (self: Camera3D) -> boolean,
	isActive: (self: Camera3D) -> boolean,
}

export type Environment3DMode = "solid" | "gradient" | "equirectangular"

-- A scene environment renders before 3D geometry and ignores camera
-- translation. `texture`/`texture_path` expect an equirectangular panorama.
export type Environment3D = ComponentInstance & {
	__neolove_component: "Environment3D",
	NEOLOVE_RENDERING: boolean,
	enabled: boolean,
	mode: Environment3DMode,
	color: Color4Value,
	top_color: Color4Value,
	bottom_color: Color4Value,
	texture: ImageHandle?,
	texture_path: string,
	rotation: number,
	intensity: number,
}

export type Skybox3D = Environment3D

export type MeshRenderer3D = ComponentInstance & {
	__neolove_component: "MeshRenderer3D",
	NEOLOVE_RENDERING: boolean,
	mesh: MeshHandle?,
	mesh_path: string,
	-- An imported path takes precedence. Use "none" for a script-assigned mesh.
	primitive: PrimitiveMeshKind | "none" | string,
	primitive_size_x: number,
	primitive_size_y: number,
	primitive_size_z: number,
	primitive_radius: number,
	primitive_height: number,
	primitive_segments: number,
	primitive_rings: number,
	-- Assign an ImageHandle here to texture the mesh immediately.
	texture: ImageHandle?,
	-- Reserved authoring metadata; normal mapping is not rendered yet.
	normal_texture: ImageHandle?,
	color: Color4Value,
	-- Reserved authoring metadata; current 3D shading is diffuse/emissive.
	metallic: number,
	roughness: number,
	visible: boolean,
	-- Reserved authoring metadata; 3D shadow maps are not rendered yet.
	casts_shadows: boolean,
	receives_shadows: boolean,
	double_sided: boolean,
	shader: ShaderHandle?,
	animation: string,
	animation_autoplay: boolean,
	animation_looping: boolean,
	animation_playing: boolean,
	-- Component-managed playback currently accepts finite non-negative speeds.
	animation_speed: number,
	playAnimation: (self: MeshRenderer3D, clip: string?) -> (),
	PlayAnimation: (self: MeshRenderer3D, clip: string?) -> (),
	pauseAnimation: (self: MeshRenderer3D) -> (),
	PauseAnimation: (self: MeshRenderer3D) -> (),
	stopAnimation: (self: MeshRenderer3D) -> (),
	StopAnimation: (self: MeshRenderer3D) -> (),
}

export type Light3DKind = "point" | "spot" | "directional" | "sun" | "spotlight"

export type Light3D = ComponentInstance & {
	__neolove_component: "Light3D",
	NEOLOVE_RENDERING: boolean,
	kind: Light3DKind,
	color: Color4Value,
	intensity: number,
	range: number,
	spot_angle: number,
	spot_softness: number,
	-- Reserved authoring metadata; 3D shadow maps are not rendered yet.
	casts_shadows: boolean,
	shadow_bias: number,
	visible: boolean,
}

export type ParticleEmissionShape3D = "point" | "box" | "sphere" | "cone"

export type ParticleSystem3D = ComponentInstance & {
	__neolove_component: "ParticleSystem3D",
	NEOLOVE_RENDERING: boolean,
	enabled: boolean,
	visible: boolean,
	playing: boolean,
	looping: boolean,
	duration: number,
	emission_rate: number,
	max_particles: number,
	shape: ParticleEmissionShape3D,
	box_size_x: number,
	box_size_y: number,
	box_size_z: number,
	sphere_radius: number,
	cone_angle: number,
	cone_length: number,
	direction_x: number,
	direction_y: number,
	direction_z: number,
	spread: number,
	lifetime: number,
	lifetime_min: number,
	lifetime_max: number,
	speed: number,
	speed_min: number,
	speed_max: number,
	gravity_x: number,
	gravity_y: number,
	gravity_z: number,
	drag: number,
	start_size: number,
	end_size: number,
	start_color: Color4Value,
	end_color: Color4Value,
	start_rotation: number,
	angular_velocity: number,
	texture: ImageHandle?,
	particle_count: number,
	seed: number,
	play: (self: ParticleSystem3D) -> (),
	Play: (self: ParticleSystem3D) -> (),
	pause: (self: ParticleSystem3D) -> (),
	Pause: (self: ParticleSystem3D) -> (),
	stop: (self: ParticleSystem3D) -> (),
	Stop: (self: ParticleSystem3D) -> (),
	emit: (self: ParticleSystem3D, count: number?) -> (),
	Emit: (self: ParticleSystem3D, count: number?) -> (),
}

-- Lightweight velocity/gravity integration with automatic positional and
-- velocity response for exact primitive contacts. Bounds-only contacts (mesh,
-- capsule-box, or non-uniform round shapes) are reported but never resolved.
export type Rigidbody3D = ComponentInstance & {
	__neolove_component: "Rigidbody3D",
	enabled: boolean,
	is_static: boolean,
	-- Reserved metadata until the lightweight solver gains mass-coupled impulses.
	mass: number,
	gravity_scale: number,
	linear_damping: number,
	angular_damping: number,
	velocity_x: number,
	velocity_y: number,
	velocity_z: number,
	angular_velocity_x: number,
	angular_velocity_y: number,
	angular_velocity_z: number,
	freeze_x: boolean,
	freeze_y: boolean,
	freeze_z: boolean,
	freeze_rotation_x: boolean,
	freeze_rotation_y: boolean,
	freeze_rotation_z: boolean,
	-- Reserved metadata; continuous collision detection is not implemented yet.
	continuous_collision: boolean,
	auto_resolve: boolean,
	contact_slop: number,
	onContact: Physics3DContactCallback?,
	onTrigger: Physics3DContactCallback?,
	setOnContact: (self: Rigidbody3D, callback: Physics3DContactCallback?) -> (),
	setOnTrigger: (self: Rigidbody3D, callback: Physics3DContactCallback?) -> (),
}

export type Collider3DShape = "box" | "sphere" | "capsule" | "mesh"

export type Collider3D = ComponentInstance & {
	__neolove_component: "Collider3D",
	enabled: boolean,
	is_trigger: boolean,
	shape: Collider3DShape,
	mesh: MeshHandle?,
	mesh_path: string,
	-- Reserved metadata; mesh raycasts always use the exact triangle BVH.
	convex: boolean,
	size_x: number,
	size_y: number,
	size_z: number,
	radius: number,
	height: number,
	offset_x: number,
	offset_y: number,
	offset_z: number,
	restitution: number,
	friction: number,
	non_physics: boolean,
	layer: number,
	mask: number,
}

export type Shape2DShape = "box" | "circle" | "triangle" | "right_triangle" | "righttriangle" | "rightangledtriangle"
export type TriangleCorner = "bl" | "br" | "tl" | "tr" | "bottomright" | "rightbottom" | "topleft" | "lefttop" | "topright" | "righttop"

export type Shape2D = BaseDrawableComponent & {
	shape: Shape2DShape,
	triangle_corner: TriangleCorner,
	offset_x: number,
	offset_y: number,
	size_x: number,
	size_y: number,
}

-- 2D lighting. Emits light into the lighting compositor when the `lighting`
-- system is enabled. Positions and aim come from the entity transform.
export type Light2DKind = "point" | "spot" | "directional"

export type Light2D = ComponentInstance & {
	__neolove_component: "Light2D",
	NEOLOVE_RENDERING: boolean,
	-- "point" radiates in every direction, "spot" is a cone aimed by the
	-- entity's rotation, "directional" floods the scene from one direction.
	kind: Light2DKind,
	color: Color4Value,
	intensity: number,
	-- Reach in pixels for point/spot lights.
	radius: number,
	-- Distance-attenuation exponent: 1 linear, 2 quadratic, and so on.
	falloff: number,
	-- Extra aim rotation in degrees added to the entity rotation (spot/directional).
	angleOffset: number,
	-- Full cone width in degrees for spot lights.
	coneAngle: number,
	-- 0 is a hard cone edge, 1 fades across the whole cone.
	coneSoftness: number,
	castsShadows: boolean,
	-- Per-light penumbra in pixels. Negative means "use lighting.setShadows".
	shadowSoftness: number,
	visible: boolean,
}

-- Blocks light and contributes ambient occlusion using the entity's bounds.
export type LightOccluderShape = "box" | "circle"

export type LightOccluder2D = ComponentInstance & {
	__neolove_component: "LightOccluder2D",
	NEOLOVE_RENDERING: boolean,
	shape: LightOccluderShape,
	visible: boolean,
}

export type ParticleEmitterShape = "point" | "box" | "circle"
export type ParticleColorKeypoint = { time: number, color: Color4Value }
export type ParticleNumberKeypoint = { time: number, value: number }

export type ParticleSystem2D = BaseDrawableComponent & {
	__neolove_component: "ParticleSystem2D",
	image: ImageHandle?,
	playing: boolean,
	looping: boolean,
	duration: number,
	emission_rate: number,
	max_particles: number,
	lifetime: number,
	speed: number,
	direction: number,
	spread: number,
	start_size: number,
	end_size: number,
	start_color: Color4Value,
	end_color: Color4Value,
	color_sequence: { ParticleColorKeypoint },
	transparency_sequence: { ParticleNumberKeypoint },
	shape: ParticleEmitterShape,
	radius: number,
	gravity_x: number,
	gravity_y: number,
	particle_count: number,
	play: (self: ParticleSystem2D) -> (),
	Play: (self: ParticleSystem2D) -> (),
	pause: (self: ParticleSystem2D) -> (),
	Pause: (self: ParticleSystem2D) -> (),
	stop: (self: ParticleSystem2D) -> (),
	Stop: (self: ParticleSystem2D) -> (),
	emit: (self: ParticleSystem2D, count: number?) -> (),
	Emit: (self: ParticleSystem2D, count: number?) -> (),
}

export type AnimationController = ComponentInstance & {
	__neolove_component: "AnimationController",
	animation: AnimationClip?,
	autoplay: boolean,
	looping: boolean,
	playing: boolean,
	speed: number,
	play: (self: AnimationController) -> (),
	Play: (self: AnimationController) -> (),
	pause: (self: AnimationController) -> (),
	Pause: (self: AnimationController) -> (),
	stop: (self: AnimationController) -> (),
	Stop: (self: AnimationController) -> (),
}

export type SpatialSound2D = ComponentInstance & {
	__neolove_component: "SpatialSound2D",
	sound: SoundHandle?,
	volume: number,
	looping: boolean,
	autoplay: boolean,
	play: (self: SpatialSound2D) -> boolean,
	Play: (self: SpatialSound2D) -> boolean,
	stop: (self: SpatialSound2D) -> (),
	Stop: (self: SpatialSound2D) -> (),
}

export type TextScaleMode = "none" | "fit" | "fit_width" | "fit_height"
export type TextAlignX = "left" | "center" | "right"
export type TextAlignY = "top" | "center" | "bottom"
export type TextWrapMode = "none" | "word" | "char"
export type TextBoundsMode = "content" | "entity" | "box" | "bounds"

export type TextFontOptions = {
	path: string?,
	file: string?,
	source: string?,
	builtin: string?,
	name: string?,
}

export type TextFont = string | TextFontOptions

export type UiTextStyle = {
	scale: number,
	min_scale: number,
	align_x: TextAlignX,
	align_y: TextAlignY,
	text_scale: TextScaleMode,
	wrap: TextWrapMode | boolean,
	padding: number,
	padding_x: number,
	padding_y: number,
	line_spacing: number,
	letter_spacing: number,
	tab_size: number,
	tab_width: number?,
	font: TextFont?,
	antialiasing: "inherit" | "off" | "standard" | "high",
}

export type TextBox = BaseDrawableComponent & UiTextStyle & {
	text: string,
	used_scale: number,
	size_mode: TextBoundsMode,
	scale_x: number,
	scale_y: number,
	dx: number,
	dy: number,
	line_count: number,
	setBold: (self: TextBox, startIndex: number, endIndex: number) -> (),
	setItalic: (self: TextBox, startIndex: number, endIndex: number) -> (),
	setUnderline: (self: TextBox, startIndex: number, endIndex: number) -> (),
	setColor: (self: TextBox, startIndex: number, endIndex: number, color: Color4Value) -> (),
	setSize: (self: TextBox, startIndex: number, endIndex: number, scale: number) -> (),
	setFont: (self: TextBox, startIndex: number, endIndex: number, fontPath: string) -> (),
	setOffset: (self: TextBox, startIndex: number, endIndex: number, x: number, y: number) -> (),
	setPixelOffset: (self: TextBox, startIndex: number, endIndex: number, x: number, y: number) -> (),
	setCharacterOffset: (self: TextBox, charIndex: number, x: number, y: number) -> (),
	clearFormatting: (self: TextBox, startIndex: number?, endIndex: number?) -> (),
	clearAllFormatting: (self: TextBox) -> (),
	getLetterCount: (self: TextBox) -> number,
	getLetterPosition: (self: TextBox, charIndex: number) -> (number?, number?),
	getLetterBounds: (self: TextBox, charIndex: number) -> (number?, number?, number?, number?),
	getClosestLetterIndex: (self: TextBox, x: number, y: number) -> number?,
	getClosestCharacterIndex: (self: TextBox, x: number, y: number) -> number?,
}

export type TextLabel = TextBox
export type RudimentaryTextLabel = TextBox

export type TextInput = BaseDrawableComponent & UiTextStyle & {
	__neolove_component: "TextInput",
	text: string,
	placeholder: string,
	enabled: boolean,
	locked: boolean,
	hovered: boolean,
	focused: boolean,
	password: boolean,
	max_length: number,
	submit_on_enter: boolean,
	clear_on_submit: boolean,
	blur_on_submit: boolean,
	cursor_index: number,
	text_color: Color4Value,
	placeholder_color: Color4Value,
	caret_color: Color4Value,
	background_color: Color4Value,
	hover_background_color: Color4Value,
	focus_background_color: Color4Value,
	disabled_background_color: Color4Value,
	border_color: Color4Value,
	hover_border_color: Color4Value,
	focus_border_color: Color4Value,
	disabled_border_color: Color4Value,
	disabled_text_color: Color4Value,
	border_width: number,
	corner_radius: number,
	caret_width: number,
	background_image: ImageHandle?,
	icon_image: ImageHandle?,
	icon_color: Color4Value,
	icon_size: number,
	icon_gap: number,
	icon_side: "left" | "right",
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
	setBold: (self: TextInput, startIndex: number, endIndex: number) -> (),
	setItalic: (self: TextInput, startIndex: number, endIndex: number) -> (),
	setUnderline: (self: TextInput, startIndex: number, endIndex: number) -> (),
	setColor: (self: TextInput, startIndex: number, endIndex: number, color: Color4Value) -> (),
	setSize: (self: TextInput, startIndex: number, endIndex: number, scale: number) -> (),
	setFont: (self: TextInput, startIndex: number, endIndex: number, fontPath: string) -> (),
	setOffset: (self: TextInput, startIndex: number, endIndex: number, x: number, y: number) -> (),
	setPixelOffset: (self: TextInput, startIndex: number, endIndex: number, x: number, y: number) -> (),
	setCharacterOffset: (self: TextInput, charIndex: number, x: number, y: number) -> (),
	clearFormatting: (self: TextInput, startIndex: number?, endIndex: number?) -> (),
	clearAllFormatting: (self: TextInput) -> (),
	focus: (self: TextInput) -> (),
	Focus: (self: TextInput) -> (),
	blur: (self: TextInput) -> (),
	Blur: (self: TextInput) -> (),
	onChanged: ((entity: Entity, component: TextInput, text: string) -> ())?,
	onSubmit: ((entity: Entity, component: TextInput, text: string) -> ())?,
	onFocus: ((entity: Entity, component: TextInput) -> ())?,
	onBlur: ((entity: Entity, component: TextInput) -> ())?,
}

-- A customizable UI container. Defaults match Visual Studio Code's Dark+ theme.
export type Panel = BaseDrawableComponent & {
	__neolove_component: "Panel",
	background_color: Color4Value,
	border_color: Color4Value,
	border_width: number,
	corner_radius: number,
	background_image: ImageHandle?,
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
}
export type Frame = Panel

-- An interactive button. Every state colour (including hover) is configurable.
export type Button = BaseDrawableComponent & UiTextStyle & {
	__neolove_component: "Button",
	text: string,
	enabled: boolean,
	hovered: boolean,
	-- Mouse clicks or Tab grant exclusive focus; Enter/Space activate the button.
	focused: boolean,
	pressed: boolean,
	background_color: Color4Value,
	hover_background_color: Color4Value,
	pressed_background_color: Color4Value,
	disabled_background_color: Color4Value,
	border_color: Color4Value,
	hover_border_color: Color4Value,
	pressed_border_color: Color4Value,
	disabled_border_color: Color4Value,
	text_color: Color4Value,
	hover_text_color: Color4Value,
	pressed_text_color: Color4Value,
	disabled_text_color: Color4Value,
	border_width: number,
	corner_radius: number,
	icon_image: ImageHandle?,
	icon_color: Color4Value,
	icon_size: number,
	icon_gap: number,
	icon_side: "left" | "right",
	background_image: ImageHandle?,
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
	focus: (self: Button) -> (),
	Focus: (self: Button) -> (),
	blur: (self: Button) -> (),
	Blur: (self: Button) -> (),
	onClick: ((entity: Entity, component: Button) -> ())?,
	onPress: ((entity: Entity, component: Button) -> ())?,
	onRelease: ((entity: Entity, component: Button) -> ())?,
	onHoverEnter: ((entity: Entity, component: Button) -> ())?,
	onHoverLeave: ((entity: Entity, component: Button) -> ())?,
	onFocus: ((entity: Entity, component: Button) -> ())?,
	onBlur: ((entity: Entity, component: Button) -> ())?,
}

-- A draggable value slider. Hover colours for the track, fill, and thumb are
-- each configurable.
export type Slider = BaseDrawableComponent & {
	__neolove_component: "Slider",
	enabled: boolean,
	hovered: boolean,
	-- Focused sliders support arrows, Home/End, and PageUp/PageDown.
	focused: boolean,
	dragging: boolean,
	min: number,
	max: number,
	value: number,
	fraction: number,
	step: number,
	orientation: "horizontal" | "vertical",
	track_thickness: number,
	thumb_size: number,
	thumb_corner_radius: number,
	corner_radius: number,
	border_width: number,
	background_color: Color4Value,
	hover_background_color: Color4Value,
	disabled_background_color: Color4Value,
	border_color: Color4Value,
	hover_border_color: Color4Value,
	disabled_border_color: Color4Value,
	fill_color: Color4Value,
	hover_fill_color: Color4Value,
	disabled_fill_color: Color4Value,
	thumb_color: Color4Value,
	hover_thumb_color: Color4Value,
	disabled_thumb_color: Color4Value,
	background_image: ImageHandle?,
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
	setValue: (self: Slider, value: number) -> (),
	SetValue: (self: Slider, value: number) -> (),
	focus: (self: Slider) -> (),
	Focus: (self: Slider) -> (),
	blur: (self: Slider) -> (),
	Blur: (self: Slider) -> (),
	onChanged: ((entity: Entity, component: Slider, value: number) -> ())?,
	onFocus: ((entity: Entity, component: Slider) -> ())?,
	onBlur: ((entity: Entity, component: Slider) -> ())?,
}

-- A selectable dropdown with a scrollable popup menu.
export type Dropdown = BaseDrawableComponent & UiTextStyle & {
	__neolove_component: "Dropdown",
	enabled: boolean,
	open: boolean,
	hovered: boolean,
	-- Focused dropdowns support arrows/Home/End and Enter/Space/Escape.
	focused: boolean,
	hover_index: number,
	selected_index: number,
	selected_text: string,
	selected_value: string,
	scroll_index: number,
	placeholder: string,
	options: { any },
	item_height: number,
	item_corner_radius: number,
	item_icon_size: number,
	item_icon_gap: number,
	menu_gap: number,
	max_visible_items: number,
	open_upwards: boolean,
	background_color: Color4Value,
	hover_background_color: Color4Value,
	open_background_color: Color4Value,
	disabled_background_color: Color4Value,
	border_color: Color4Value,
	hover_border_color: Color4Value,
	open_border_color: Color4Value,
	disabled_border_color: Color4Value,
	text_color: Color4Value,
	disabled_text_color: Color4Value,
	menu_background_color: Color4Value,
	menu_border_color: Color4Value,
	item_background_color: Color4Value,
	item_hover_background_color: Color4Value,
	item_selected_background_color: Color4Value,
	item_text_color: Color4Value,
	item_hover_text_color: Color4Value,
	item_selected_text_color: Color4Value,
	border_width: number,
	corner_radius: number,
	background_image: ImageHandle?,
	icon_image: ImageHandle?,
	icon_color: Color4Value,
	icon_size: number,
	icon_gap: number,
	icon_side: "left" | "right",
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
	focus: (self: Dropdown) -> (),
	Focus: (self: Dropdown) -> (),
	blur: (self: Dropdown) -> (),
	Blur: (self: Dropdown) -> (),
	onChanged: ((entity: Entity, component: Dropdown, index: number, value: string) -> ())?,
	onFocus: ((entity: Entity, component: Dropdown) -> ())?,
	onBlur: ((entity: Entity, component: Dropdown) -> ())?,
}

export type ScrollListItem = {
	text: string?,
	label: string?,
	name: string?,
	value: (string | number | boolean)?,
	id: (string | number | boolean)?,
	image: ImageHandle?,
	icon: ImageHandle?,
	image_color: Color4Value?,
	icon_color: Color4Value?,
}

-- A scrollable, keyboard-navigable list with exclusive focus and captured
-- scrollbar dragging. Options may be primitive values or ScrollListItem tables.
export type ScrollList = BaseDrawableComponent & UiTextStyle & {
	__neolove_component: "ScrollList",
	enabled: boolean,
	hovered: boolean,
	focused: boolean,
	hover_index: number,
	selected_index: number,
	selected_text: string,
	selected_value: string,
	scroll_index: number,
	options: { string | number | boolean | ScrollListItem },
	empty_text: string,
	item_height: number,
	item_spacing: number,
	item_corner_radius: number,
	item_icon_size: number,
	item_icon_gap: number,
	item_padding_x: number,
	item_padding_y: number,
	show_scrollbar: boolean,
	scrollbar_width: number,
	scrollbar_dragging: boolean,
	background_color: Color4Value,
	hover_background_color: Color4Value,
	focus_background_color: Color4Value,
	disabled_background_color: Color4Value,
	border_color: Color4Value,
	hover_border_color: Color4Value,
	focus_border_color: Color4Value,
	disabled_border_color: Color4Value,
	text_color: Color4Value,
	empty_text_color: Color4Value,
	disabled_text_color: Color4Value,
	item_background_color: Color4Value,
	item_hover_background_color: Color4Value,
	item_selected_background_color: Color4Value,
	item_text_color: Color4Value,
	item_hover_text_color: Color4Value,
	item_selected_text_color: Color4Value,
	scrollbar_color: Color4Value,
	scrollbar_thumb_color: Color4Value,
	border_width: number,
	corner_radius: number,
	background_image: ImageHandle?,
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
	focus: (self: ScrollList) -> (),
	Focus: (self: ScrollList) -> (),
	blur: (self: ScrollList) -> (),
	Blur: (self: ScrollList) -> (),
	onChanged: ((entity: Entity, component: ScrollList, index: number, value: string) -> ())?,
	onFocus: ((entity: Entity, component: ScrollList) -> ())?,
	onBlur: ((entity: Entity, component: ScrollList) -> ())?,
}

export type Sprite2D = BaseDrawableComponent & {
	__neolove_component: "Sprite2D" | "Image2D",
	image: ImageHandle?,
	source_x: number?,
	source_y: number?,
	source_w: number?,
	source_h: number?,
	source_width: number?,
	source_height: number?,
	sourceX: number?,
	sourceY: number?,
	sourceW: number?,
	sourceH: number?,
	sourceWidth: number?,
	sourceHeight: number?,
}

export type Image2D = Sprite2D

export type SpriteSheet2D = BaseDrawableComponent & {
	__neolove_component: "SpriteSheet2D",
	image: ImageHandle?,
	frame_width: number,
	frame_height: number,
	columns: number,
	frame_count: number,
	spacing: number,
	margin: number,
	frame: number,
	fps: number,
	playing: boolean,
	looping: boolean,
	play: (self: SpriteSheet2D) -> (),
	pause: (self: SpriteSheet2D) -> (),
	stop: (self: SpriteSheet2D) -> (),
	setFrame: (self: SpriteSheet2D, frame: number) -> (),
	set_frame: (self: SpriteSheet2D, frame: number) -> (),
}

export type NineSliceSprite2D = BaseDrawableComponent & {
	__neolove_component: "NineSliceSprite2D",
	image: ImageHandle?,
	source_x: number?,
	source_y: number?,
	source_w: number?,
	source_h: number?,
	source_width: number?,
	source_height: number?,
	sourceX: number?,
	sourceY: number?,
	sourceW: number?,
	sourceH: number?,
	sourceWidth: number?,
	sourceHeight: number?,
	slice_left: number,
	slice_right: number,
	slice_top: number,
	slice_bottom: number,
	sliceLeft: number?,
	sliceRight: number?,
	sliceTop: number?,
	sliceBottom: number?,
}

export type TileTexture2D = BaseDrawableComponent & {
	image: ImageHandle?,
	tile_width: number,
	tile_height: number,
	offset_x: number,
	offset_y: number,
}

export type Tilemap2D = BaseDrawableComponent & {
	image: ImageHandle?,
	map_width: number,
	map_height: number,
	tile_width: number,
	tile_height: number,
	tiles: string | { number },
	spacing: number,
	margin: number,
}

export type Spritebox2D = ComponentInstance & {
	__neolove_component: "Spritebox2D",
	computed: boolean,
	alpha_threshold: number,
	rect_count: number,
	bounds_x: number,
	bounds_y: number,
	bounds_w: number,
	bounds_h: number,
	ComputeSpritebox: (self: Spritebox2D) -> boolean,
	computeSpritebox: (self: Spritebox2D) -> boolean,
	IsInside: (self: Spritebox2D, x: number, y: number) -> boolean,
	isInside: (self: Spritebox2D, x: number, y: number) -> boolean,
	IsIntersecting: (self: Spritebox2D, other: Entity | Spritebox2D) -> boolean,
	isIntersecting: (self: Spritebox2D, other: Entity | Spritebox2D) -> boolean,
}

export type Collider2D = ComponentInstance & {
	__neolove_component: "Collider2D",
	enabled: boolean,
	is_trigger: boolean,
	non_physics: boolean,
	offset_x: number,
	offset_y: number,
	size_x: number,
	size_y: number,
	shape: string,
	triangle_corner: TriangleCorner,
	restitution: number,
	friction: number,
	touching: boolean,
	last_hit_id: number,
	onCollisionEnter: CollisionCallback?,
	onCollisionStay: CollisionCallback?,
	onCollisionExit: CollisionCallback?,
	onTriggerEnter: CollisionCallback?,
	onTriggerStay: CollisionCallback?,
	onTriggerExit: CollisionCallback?,
	setOnCollisionEnter: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnCollisionStay: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnCollisionExit: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnTriggerEnter: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnTriggerStay: (self: Collider2D, callback: CollisionCallback?) -> (),
	setOnTriggerExit: (self: Collider2D, callback: CollisionCallback?) -> (),
}

export type RigidbodyBoundsMode = "none" | "window"

export type Rigidbody2D = ComponentInstance & {
	__neolove_component: "Rigidbody2D",
	velocity_x: number,
	velocity_y: number,
	force_x: number,
	force_y: number,
	acceleration_x: number,
	acceleration_y: number,
	gravity_x: number,
	gravity_y: number,
	gravity_scale: number,
	mass: number,
	inertia: number,
	linear_damping: number,
	angular_damping: number,
	restitution: number,
	friction: number,
	sleep_epsilon: number,
	bounds_mode: RigidbodyBoundsMode | string,
	freeze_x: boolean,
	freeze_y: boolean,
	freeze_rotation: boolean,
	is_static: boolean,
	collision_enabled: boolean,
	grounded: boolean,
	max_speed: number,
	max_angular_speed: number,
	angular_velocity: number,
	torque: number,
	addForce: (self: Rigidbody2D, fx: number, fy: number) -> (),
	addImpulse: (self: Rigidbody2D, ix: number, iy: number) -> (),
	addTorque: (self: Rigidbody2D, torque: number) -> (),
	addAngularImpulse: (self: Rigidbody2D, impulse: number) -> (),
	setVelocity: (self: Rigidbody2D, vx: number, vy: number) -> (),
	getVelocity: (self: Rigidbody2D) -> (number, number),
	setAngularVelocity: (self: Rigidbody2D, omega: number) -> (),
	getAngularVelocity: (self: Rigidbody2D) -> number,
	setGravity: (self: Rigidbody2D, gx: number, gy: number) -> (),
}

export type Bolt2D = ComponentInstance & {
	__neolove_component: "Bolt2D",
	enabled: boolean,
	target_entity: Entity?,
	target: Entity?,
	x: number,
	y: number,
	offset_x: number,
	offset_y: number,
	strength: number,
	contacts_enabled: boolean,
	current_force: number,
	force: number,
	attach: (self: Bolt2D, targetEntity: Entity) -> (),
	link: (self: Bolt2D, targetEntity: Entity) -> (),
}

export type LegacyBolt2D = ComponentInstance & {
	__neolove_component: "LegacyBolt2D",
	enabled: boolean,
	target_entity: Entity?,
	target: Entity?,
	x: number,
	y: number,
	offset_x: number,
	offset_y: number,
	strength: number,
	contacts_enabled: boolean,
	current_force: number,
	force: number,
	attach: (self: LegacyBolt2D, targetEntity: Entity) -> (),
	link: (self: LegacyBolt2D, targetEntity: Entity) -> (),
}

export type Rope2D = ComponentInstance & {
	__neolove_component: "Rope2D",
	enabled: boolean,
	entity_a: Entity?,
	entity_b: Entity?,
	min_length: number,
	max_length: number,
	stiffness: number,
	damping: number,
	break_force: number,
	current_length: number,
	tension: number,
	snapped: boolean,
	link: (self: Rope2D, entityA: Entity, entityB: Entity) -> (),
}

export type Physics3DRaycastOptions = {
	layer: number?,
	mask: number?,
	include_triggers: boolean?,
}

export type Physics3DRaycastHit = {
	entity_id: number,
	distance: number,
	x: number,
	y: number,
	z: number,
	normal_x: number,
	normal_y: number,
	normal_z: number,
	front_face: boolean,
	is_trigger: boolean,
	-- Present for mesh colliders; indices are one-based for Luau.
	triangle_index: number?,
	barycentric: { number }?,
}

export type Physics3DBroadphasePair = {
	first_id: number,
	second_id: number,
	has_trigger: boolean,
}

export type Physics3DContactQuality = "exact" | "bounds"

export type Physics3DContactOptions = {
	include_triggers: boolean?,
	include_bounds: boolean?,
	entity_id: number?,
}

export type Physics3DContact = {
	first_id: number,
	second_id: number,
	normal_x: number,
	normal_y: number,
	normal_z: number,
	penetration: number,
	x: number,
	y: number,
	z: number,
	is_trigger: boolean,
	quality: Physics3DContactQuality,
	exact: boolean,
	can_resolve: boolean,
}

export type Physics3DBodyContact = {
	self_id: number,
	other_id: number,
	normal_x: number,
	normal_y: number,
	normal_z: number,
	penetration: number,
	x: number,
	y: number,
	z: number,
	is_trigger: boolean,
	quality: Physics3DContactQuality,
	exact: boolean,
	can_resolve: boolean,
}

export type Physics3DContactCallback = (contact: Physics3DBodyContact) -> ()

export type Physics3DResolveOptions = {
	response: boolean?,
	correction_percent: number?,
	slop: number?,
}

export type Physics3DResolveResult = {
	contacts: { Physics3DBodyContact },
	contact_count: number,
	trigger_count: number,
	exact_count: number,
	bounds_count: number,
	resolved_count: number,
	grounded: boolean,
	correction_x: number,
	correction_y: number,
	correction_z: number,
}

export type Physics3DModule = {
	raycast: (
		origin_x: number,
		origin_y: number,
		origin_z: number,
		dir_x: number,
		dir_y: number,
		dir_z: number,
		max_distance: number?,
		options: Physics3DRaycastOptions?
	) -> Physics3DRaycastHit?,
	-- Runs deterministic broadphase + primitive narrowphase. `bounds` contacts
	-- are conservative candidates, not exact mesh/ellipsoid manifolds.
	contacts: (options: Physics3DContactOptions?) -> { Physics3DContact },
	-- Resolves only exact, non-trigger primitive contacts for one body. Called
	-- automatically by Rigidbody3D when `auto_resolve` is true.
	resolveBody: (
		entity: Entity,
		body: Rigidbody3D,
		options: Physics3DResolveOptions?
	) -> Physics3DResolveResult,
	broadphasePairs: () -> { Physics3DBroadphasePair },
}

-- Quality/resolution of the 2D light map.
export type LightQuality = "low" | "medium" | "high" | "ultra"

-- Global 2D lighting controls. Settings persist across frames; toggle the whole
-- system with `setEnabled`. Lights and occluders come from `Light2D` /
-- `LightOccluder2D` components. Software renders the map directly; Vulkan
-- composites a CPU-generated map on the GPU; web capture uses the CPU path.
export type LightingModule = {
	setEnabled: (enabled: boolean?) -> (),
	enable: () -> (),
	disable: () -> (),
	isEnabled: () -> boolean,
	-- Base light present everywhere. Full white at intensity 1 leaves the scene
	-- unlit; lower it to create darkness for lights to cut through.
	setAmbient: (color: Color4Value, intensity: number?) -> (),
	setAmbientIntensity: (intensity: number) -> (),
	getAmbient: () -> (Color4Value, number),
	-- Darken ambient light near occluder edges. radius/intensity/samples optional.
	setAmbientOcclusion: (enabled: boolean?, radius: number?, intensity: number?, samples: number?) -> (),
	-- Toggle occluder shadows. `softness` (pixels) adds a penumbra; 0 is hard.
	setShadows: (enabled: boolean?, softness: number?) -> (),
	-- Glow added where light exceeds full brightness on software/web CPU paths.
	-- The native Vulkan light-map encoding currently clamps over-bright values.
	setBloom: (amount: number) -> (),
	-- Output multiplier applied after lighting.
	setExposure: (value: number) -> (),
	setQuality: (quality: LightQuality) -> (),
	getQuality: () -> LightQuality,
	-- The light color reaching a screen-space position (from the last completed
	-- frame), or nil when the point is off-screen. White when lighting is off.
	sample: (x: number, y: number) -> Color4Value?,
	getAt: (x: number, y: number) -> Color4Value?,
	sampleAt: (x: number, y: number) -> Color4Value?,
	reset: () -> (),
}

export type PostProcessEffectName =
	"bloom"
	| "pixelate"
	| "pixelated"
	| "pixelation"
	| "chromatic_aberration"
	| "chromatic_abberation"
	| "chromatic"
	| "motion_blur"
	| "motionblur"
	| "quantization"
	| "quantize"
	| "posterize"
	| "vignette"
	| "grayscale"
	| "greyscale"
	| "invert"
	| "negative"
	| "color_adjust"
	| "brightness_contrast_saturation"
	| "bcs"
	| "exposure"
	| "tonemap"
	| "exposure_tonemap"

-- Only fields used by an effect are read. All effect passes are ordered and
-- use one-based stack indices in Luau.
export type PostProcessOptions = {
	enabled: boolean?,
	threshold: number?,
	intensity: number?,
	radius: number?,
	block_size: number?,
	offset_pixels: number?,
	angle_degrees: number?,
	strength: number?,
	levels: number?,
	dither_strength: number?,
	softness: number?,
	amount: number?,
	brightness: number?,
	contrast: number?,
	saturation: number?,
	exposure: number?,
	operator: ("none" | "off" | "reinhard" | "aces" | "aces_filmic")?,
	gamma: number?,
}

export type PostProcessModule = {
	setEnabled: (enabled: boolean?) -> (),
	isEnabled: () -> boolean,
	add: (kind: PostProcessEffectName, options: PostProcessOptions?) -> number,
	remove: (index: number) -> boolean,
	move: (fromIndex: number, toIndex: number) -> boolean,
	setPassEnabled: (index: number, enabled: boolean) -> boolean,
	clear: () -> (),
	resetHistory: () -> (),
	count: () -> number,
}

export type Environment3DModule = {
	setSolid: (color: Color4Value) -> (),
	setGradient: (top: Color4Value, bottom: Color4Value) -> (),
	setEquirectangular: (image: ImageHandle, rotationDegrees: number?) -> (),
	setEnabled: (enabled: boolean) -> (),
	setIntensity: (intensity: number) -> (),
	clear: () -> (),
}

export type CoreModule = {
	Rect2D: Rect2D,
	Light2D: Light2D,
	LightOccluder2D: LightOccluder2D,
	EntityScaler: EntityScaler,
	Camera: Camera,
	Camera3D: Camera3D,
	Environment3D: Environment3D,
	Skybox3D: Skybox3D,
	MeshRenderer3D: MeshRenderer3D,
	Light3D: Light3D,
	ParticleSystem3D: ParticleSystem3D,
	Rigidbody3D: Rigidbody3D,
	Collider3D: Collider3D,
	Shape2D: Shape2D,
	ParticleSystem2D: ParticleSystem2D,
	AnimationController: AnimationController,
	SpatialSound2D: SpatialSound2D,
	TextBox: TextBox,
	TextLabel: TextLabel,
	RudimentaryTextLabel: RudimentaryTextLabel,
	TextInput: TextInput,
	Panel: Panel,
	Frame: Frame,
	Button: Button,
	Slider: Slider,
	Dropdown: Dropdown,
	ScrollList: ScrollList,
	Sprite2D: Sprite2D,
	SpriteSheet2D: SpriteSheet2D,
	Image2D: Image2D,
	NineSliceSprite2D: NineSliceSprite2D,
	["9SliceSprite2D"]: NineSliceSprite2D,
	TileTexture2D: TileTexture2D,
	Tilemap2D: Tilemap2D,
	Spritebox2D: Spritebox2D,
	Collider2D: Collider2D,
	Rigidbody2D: Rigidbody2D,
	Bolt2D: Bolt2D,
	LegacyBolt2D: LegacyBolt2D,
	Rope2D: Rope2D,
	String2D: Rope2D,
}

-- A seedable random-number generator. Create independent, reproducible streams
-- with `Rng.new(seed)` instead of sharing `math.random`'s hidden global state.
export type RngInstance = {
	-- Float in [0, 1).
	next: (self: RngInstance) -> number,
	-- number() -> [0,1); number(max) -> [0,max); number(min,max) -> [min,max).
	number: (self: RngInstance, min: number?, max: number?) -> number,
	float: (self: RngInstance, min: number?, max: number?) -> number,
	range: (self: RngInstance, min: number?, max: number?) -> number,
	-- Inclusive integer. integer(max) -> [1,max]; integer(min,max) -> [min,max].
	integer: (self: RngInstance, min: number, max: number?) -> number,
	int: (self: RngInstance, min: number, max: number?) -> number,
	-- True with probability p (default 0.5).
	boolean: (self: RngInstance, p: number?) -> boolean,
	bool: (self: RngInstance, p: number?) -> boolean,
	sign: (self: RngInstance) -> number,
	angle: (self: RngInstance) -> number,
	unit: (self: RngInstance) -> (number, number),
	pick: (self: RngInstance, list: { any }) -> any,
	shuffle: (self: RngInstance, list: { any }) -> { any },
	seed: (self: RngInstance, seed: number) -> (),
	clone: (self: RngInstance) -> RngInstance,
	Clone: (self: RngInstance) -> RngInstance,
}

export type RngModule = {
	new: (seed: number?) -> RngInstance,
	fromString: (text: string) -> RngInstance,
} & ((seed: number?) -> RngInstance)

declare function Color4(r: number, g: number, b: number, a: number?): Color4Value
declare function Inspector<T>(defaultValue: T, max: number?, fractional: boolean?): T
-- Inspector reference placeholders. The visual editor replaces these with a
-- selected scene entity/component when it generates the scene.
declare IEntity: Entity
declare IComponent: ComponentInstance
declare IImage: ImageHandle
declare IAudio: SoundHandle
declare IShader: ShaderHandle
declare IAnimation: AnimationClip
-- Register the current behaviour module in the editor's "Add Component" picker,
-- so it can be attached to entities like a core component. Call it at module
-- scope, e.g. `IComponentPicker(Behaviour)`.
declare function IComponentPicker(behaviour: any): ()
declare function die(): ()
declare function softrequire(modulePathOrSource: string, allowedModules: { [string]: any } | { string }?): any

declare app: AppModule
declare input: InputModule
declare userInput: InputModule
declare assets: AssetsModule
declare audio: AudioModule
declare media: MediaModule
declare microphone: MicrophoneModule
declare fs: FsModule
declare android: AndroidModule
declare mobile: MobileModule
declare http: HttpModule
declare commands: CommandsModule
declare command: CommandsModule
declare servers: ServersModule
declare shaders: ShadersModule
declare ecs: EcsModule
declare prefabs: PrefabsModule
declare prefab: PrefabsModule
declare tweening: TweeningModule
declare tween: TweeningModule
declare animation: AnimationModule
declare animations: AnimationModule
declare transform: TransformModule
declare transforms: TransformModule
declare core: CoreModule
declare lighting: LightingModule
declare physics3d: Physics3DModule
declare physics3D: Physics3DModule
declare postprocess: PostProcessModule
declare postProcess: PostProcessModule
declare environment3d: Environment3DModule
declare environment3D: Environment3DModule
declare skybox: Environment3DModule
declare Rng: RngModule
declare async: AsyncModule

declare mouse: Vec2
declare window: Vec2

return nil
