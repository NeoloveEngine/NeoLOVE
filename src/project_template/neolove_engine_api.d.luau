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

export type PositionPivot = "center" | "top_right"

export type EntityListenEvent = "leftClick" | "rightClick" | "middleClick" | "scrollUp" | "scrollDown"

export type EntityListenInfo = {
	kind: EntityListenEvent,
	type: EntityListenEvent,
	button: "left" | "right" | "middle"?,
	x: number,
	y: number,
	mouseX: number,
	mouseY: number,
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
	anchor_x: number,
	anchor_y: number,
	pivot_x: number?,
	pivot_y: number?,
	rotation: number,
	rotation_pivot: string,
	rotation_pivot_x: number?,
	rotation_pivot_y: number?,
	position_pivot: PositionPivot?,
	z: number,
	size_x: number,
	size_y: number,
	scale: number,
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
	setMaxFps: (fps: number?) -> (),
	getMaxFps: () -> number?,
	setShowFps: (enabled: boolean?) -> (),
	getShowFps: () -> boolean,
	nearestNeighborScaling: boolean,
	setNearestNeighborScaling: (enabled: boolean?) -> (),
	getNearestNeighborScaling: () -> boolean,
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
}

export type AssetsModule = {
	loadImage: (path: string) -> ImageHandle,
	newImage: (width: number, height: number, color: Color4Value?) -> ImageHandle,
	loadSound: (path: string) -> SoundHandle,
	newSound: (sampleRate: number, channels: number, len: number, fill: number?) -> SoundHandle,
	unloadImage: (value: string | ImageHandle) -> boolean,
	unloadSound: (value: string | SoundHandle) -> boolean,
	gc: () -> (number, number),
}

export type AudioModule = {
	play: (sound: SoundHandle, looped: boolean?, volume: number?) -> (),
	playOnce: (sound: SoundHandle, volume: number?) -> (),
	stop: (sound: SoundHandle) -> (),
	setVolume: (sound: SoundHandle, volume: number) -> (),
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
	openFilePicker: () -> string?,
	openFolderPicker: () -> string?,
	getDataDirectory: () -> string,
	dataPath: (path: string) -> string,
	readFile: (path: string) -> string,
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
}

export type HostedServerHandle = {
	client: ServerClientHandle,
	port: number,
	url: string,
	stop: () -> boolean,
	getPort: () -> number,
	getUrl: () -> string,
}

export type ServersModule = {
	host: (scriptPath: string, port: number, options: ServerHostOptions?) -> HostedServerHandle,
	connect: (url: string) -> ServerClientHandle,
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
	fromSource: (vertexSource: string, fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
	fromFragmentSource: (fragmentSource: string, options: ShaderLoadOptions?) -> ShaderHandle,
}

export type TransformModule = {
	getWorldPosition: (entity: Entity) -> (number, number),
	getWorldRotation: (entity: Entity) -> number,
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
	anchor_x: number?,
	anchor_y: number?,
	pivot_x: number?,
	pivot_y: number?,
	rotation: number?,
	rotation_pivot: string?,
	rotation_pivot_x: number?,
	rotation_pivot_y: number?,
	position_pivot: PositionPivot?,
	z: number?,
	size_x: number?,
	size_y: number?,
	scale: number?,
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

export type BaseDrawableComponent = ComponentInstance & {
	NEOLOVE_RENDERING: boolean,
	color: Color4Value,
	shader: ShaderHandle?,
	visible: boolean,
}

export type Rect2D = BaseDrawableComponent

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
	setColor: (self: TextBox, startIndex: number, endIndex: number, color: Color4) -> (),
	setSize: (self: TextBox, startIndex: number, endIndex: number, scale: number) -> (),
	setFont: (self: TextBox, startIndex: number, endIndex: number, fontPath: string) -> (),
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

export type CoreModule = {
	Rect2D: Rect2D,
	Shape2D: Shape2D,
	TextBox: TextBox,
	TextLabel: TextLabel,
	RudimentaryTextLabel: RudimentaryTextLabel,
	Sprite2D: Sprite2D,
	Image2D: Image2D,
	NineSliceSprite2D: NineSliceSprite2D,
	["9SliceSprite2D"]: NineSliceSprite2D,
	TileTexture2D: TileTexture2D,
	Spritebox2D: Spritebox2D,
	Collider2D: Collider2D,
	Rigidbody2D: Rigidbody2D,
	Bolt2D: Bolt2D,
	LegacyBolt2D: LegacyBolt2D,
	Rope2D: Rope2D,
	String2D: Rope2D,
}

declare function Color4(r: number, g: number, b: number, a: number?): Color4Value
declare function die(): ()
declare function softrequire(modulePathOrSource: string, allowedModules: { [string]: any } | { string }?): any

declare app: AppModule
declare input: InputModule
declare userInput: InputModule
declare assets: AssetsModule
declare audio: AudioModule
declare fs: FsModule
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
declare transform: TransformModule
declare transforms: TransformModule
declare core: CoreModule
declare async: AsyncModule

declare mouse: Vec2
declare window: Vec2

return nil
