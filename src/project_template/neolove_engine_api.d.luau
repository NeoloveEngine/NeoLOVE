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

export type ComponentInstance = {
	entity: Entity?,
	awake: ((entity: Entity, component: ComponentInstance) -> ())?,
	update: ((entity: Entity, component: ComponentInstance, dt: number) -> ())?,
	destroy: ((entity: Entity, component: ComponentInstance) -> ())?,
	onDestroy: ((entity: Entity, component: ComponentInstance) -> ())?,
	NEOLOVE_RENDERING: boolean?,
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
	unload: (self: SoundHandle) -> (),
	isUnloaded: (self: SoundHandle) -> boolean,
}

export type RaycastHit = {
	entity: Entity,
	id: number,
	distance: number,
	x: number,
	y: number,
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
	is_file: boolean,
	is_dir: boolean,
}

export type FsModule = {
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

export type HttpHeaders = { [string]: string }

export type HttpResponse = {
	ok: boolean,
	url: string,
	status: number?,
	body: string,
	error: string?,
	headers: HttpHeaders,
}

export type HttpModule = {
	request: (url: string, callback: (response: HttpResponse) -> ()) -> number,
	get: (url: string, callback: (response: HttpResponse) -> ()) -> number,
	_poll: () -> (),
}

export type CommandRunResult = {
	ok: boolean,
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
	addComponent: <T>(entity: Entity, component: T) -> T,
	removeComponent: (entity: Entity, target: number | ComponentInstance) -> boolean,
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
}

export type TextLabel = TextBox
export type RudimentaryTextLabel = TextBox

export type Image2D = BaseDrawableComponent & {
	image: ImageHandle?,
}

export type TileTexture2D = BaseDrawableComponent & {
	image: ImageHandle?,
	tile_width: number,
	tile_height: number,
	offset_x: number,
	offset_y: number,
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
	Image2D: Image2D,
	TileTexture2D: TileTexture2D,
	Collider2D: Collider2D,
	Rigidbody2D: Rigidbody2D,
	Rope2D: Rope2D,
	String2D: Rope2D,
}

declare function Color4(r: number, g: number, b: number, a: number?): Color4Value
declare function die(): ()
declare function softrequire(modulePath: string, allowedModules: { [string]: any } | { string }?): any

declare app: AppModule
declare input: InputModule
declare userInput: InputModule
declare assets: AssetsModule
declare audio: AudioModule
declare fs: FsModule
declare http: HttpModule
declare commands: CommandsModule
declare command: CommandsModule
declare shaders: ShadersModule
declare ecs: EcsModule
declare transform: TransformModule
declare transforms: TransformModule
declare core: CoreModule

declare mouse: Vec2
declare window: Vec2

return nil
