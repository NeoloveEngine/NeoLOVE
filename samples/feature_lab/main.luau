local helpers = require("./lab_helpers")
local inlineModule = softrequire("return { marker = 'inline softrequire' }", {})

app.bg = Color4(7, 11, 18, 255)
app.setMaxFps(120)
app.setShowFps(true)
app.setNearestNeighborScaling(true)

local WHITE = Color4(242, 246, 255, 255)
local MUTED = Color4(145, 161, 188, 255)
local CYAN = Color4(65, 214, 224, 255)
local BLUE = Color4(76, 135, 255, 255)
local GREEN = Color4(73, 211, 142, 255)
local AMBER = Color4(255, 190, 76, 255)
local PANEL = Color4(18, 27, 42, 245)
local PANEL_ALT = Color4(23, 34, 53, 245)
local TRANSPARENT = Color4(0, 0, 0, 0)

local results = {}
local logs = {}
local counters = {
	awake = 0,
	update = 0,
	destroy = 0,
	systemAwake = 0,
	systemUpdate = 0,
	systemLate = 0,
	systemFixed = 0,
	collisionEnter = 0,
	collisionStay = 0,
	collisionExit = 0,
	triggerEnter = 0,
	triggerStay = 0,
	triggerExit = 0,
	echo = 0,
}

local function addLog(message)
	local text = helpers.short(message, 88)
	print("[feature_lab] " .. text)
	table.insert(logs, 1, text)
	while #logs > 7 do
		table.remove(logs)
	end
end

local function setResult(name, ok, detail)
	results[name] = {
		ok = ok,
		detail = helpers.short(detail or (ok and "ok" or "failed"), 64),
	}
	addLog(string.format("%s: %s", name, results[name].detail))
end

local function probe(name, action)
	local ok, value = pcall(action)
	if ok then
		setResult(name, true, value)
	else
		setResult(name, false, value)
	end
	return ok, value
end

local function makeEntity(name, parent, x, y, w, h, z)
	local entity = ecs.newEntity(name, parent or ecs.root, x or 0, y or 0)
	entity.size_x = w or 1
	entity.size_y = h or 1
	entity.z = z or 0
	return entity
end

local function addShape(entity, shape, color)
	local renderer = entity:AddComponent(core.Shape2D)
	renderer.shape = shape or "box"
	renderer.color = color or WHITE
	return renderer
end

local function addRect(entity, color)
	local renderer = ecs.addComponent(entity, core.Rect2D)
	renderer.color = color or WHITE
	return renderer
end

local function addText(entity, text, scale, color, alignX, alignY)
	local label = entity:AddComponent(core.TextBox)
	label.text = text or ""
	label.scale = scale or 18
	label.min_scale = 10
	label.color = color or WHITE
	label.size_mode = "entity"
	label.text_scale = "fit"
	label.wrap = "word"
	label.align_x = alignX or "left"
	label.align_y = alignY or "top"
	label.padding_x = 8
	label.padding_y = 5
	return label
end

local function makePanel(name, x, y, w, h, color)
	local entity = makeEntity(name, ecs.root, x, y, w, h, -20)
	entity.raycastable = false
	addRect(entity, color or PANEL)
	return entity
end

local function makeLabel(name, parent, x, y, w, h, text, scale, color, alignX, alignY)
	local entity = makeEntity(name, parent, x, y, w, h, 20)
	entity.raycastable = false
	local label = addText(entity, text, scale, color, alignX, alignY)
	return entity, label
end

local generatedImage
local generatedSound
local reloadedImage
local reloadedSound

local function buildImage()
	local image = assets.newImage(48, 48, TRANSPARENT)
	for y = 0, 47 do
		for x = 0, 47 do
			local checker = (math.floor(x / 6) + math.floor(y / 6)) % 2 == 0
			local dx = x - 23.5
			local dy = y - 23.5
			local distance = math.sqrt(dx * dx + dy * dy)
			if distance < 21 then
				local glow = math.max(0, 1 - distance / 21)
				image:setPixel(
					x,
					y,
					Color4(
						helpers.clampByte((checker and 35 or 10) + glow * 70),
						helpers.clampByte((checker and 145 or 70) + glow * 95),
						helpers.clampByte((checker and 205 or 120) + glow * 45),
						255
					)
				)
			end
		end
	end
	image:upload()
	return image
end

local function buildSound()
	local rate = 24000
	local duration = 0.45
	local sound = assets.newSound(rate, 1, math.floor(rate * duration), 0)
	for index = 0, sound:len() - 1 do
		local time = index / rate
		local fade = math.max(0, 1 - time / duration)
		local sample = (
			math.sin(time * math.pi * 2 * 440)
			+ math.sin(time * math.pi * 2 * 660) * 0.35
		) * fade * 0.38
		sound:setSample(index, sample)
	end
	sound:upload()
	return sound
end

generatedImage = buildImage()
generatedSound = buildSound()

local function runAssetRoundTrip()
	probe("assets", function()
		fs.createDir("runtime")
		local imagePath = fs.dataPath("runtime/lab_image")
		generatedImage:save(imagePath)
		generatedSound:export("runtime/lab_sound.wav")
		reloadedImage = assets.loadImage(imagePath .. ".png")
		reloadedSound = assets.loadSound("runtime/lab_sound.wav")
		local width, height = reloadedImage:size()
		local pixel = reloadedImage:getPixel(24, 24)
		local sample = reloadedSound:getSample(0)
		reloadedImage:fill(Color4(pixel.r, pixel.g, pixel.b, 255))
		reloadedImage:upload()
		reloadedImage:export("runtime/lab_image_copy.png")
		reloadedSound:setSample(0, sample)
		reloadedSound:upload()
		reloadedSound:save("runtime/lab_sound_copy")
		local imageUnloaded = assets.unloadImage(reloadedImage)
		local soundUnloaded = assets.unloadSound(reloadedSound)
		local flags = reloadedImage:isUnloaded() and reloadedSound:isUnloaded()
		local imagePathUnloaded = assets.unloadImage("runtime/lab_image.png")
		local soundPathUnloaded = assets.unloadSound("runtime/lab_sound.wav")
		reloadedImage = nil
		reloadedSound = nil
		local images, sounds = assets.gc()
		return string.format(
			"%dx%d unload=%s/%s path=%s/%s flags=%s gc=%d/%d",
			width,
			height,
			tostring(imageUnloaded),
			tostring(soundUnloaded),
			tostring(imagePathUnloaded),
			tostring(soundPathUnloaded),
			tostring(flags),
			images,
			sounds
		)
	end)
end

local function runFsProbe()
	probe("filesystem", function()
		fs.createDir("runtime/fs/nested")
		fs.writeFile("runtime/fs/source.txt", "alpha")
		fs.appendFile("runtime/fs/source.txt", "\nbeta")
		fs.copy("runtime/fs/source.txt", "runtime/fs/nested/copy.txt")
		fs.rename("runtime/fs/nested/copy.txt", "runtime/fs/renamed.txt")
		local content = fs.readFile("runtime/fs/source.txt")
		local dataDirectory = fs.getDataDirectory()
		local sourcePath = fs.dataPath("runtime/fs/source.txt")
		local entries = fs.walk("runtime/fs", true)
		local checks = fs.exists("runtime/fs/renamed.txt")
			and fs.isFile("runtime/fs/renamed.txt")
			and fs.isDir("runtime/fs/nested")
			and fs.isFile(sourcePath)
		local removed = fs.removeFile("runtime/fs/renamed.txt")
		return string.format(
			"read=%s walk=%d checks=%s removed=%s data=%s",
			content,
			#entries,
			tostring(checks),
			tostring(removed),
			helpers.short(dataDirectory, 24)
		)
	end)
end

local function runServerHelpers()
	probe("server helpers", function()
		local source = {
			name = "NeoLOVE",
			nested = { ok = true, number = 42 },
			bytes = buffer.fromstring("lab"),
		}
		local payload = servers.serializeTable(source)
		local decoded = servers.deserialize_table(payload)
		local uuid4 = servers.generateUuid4()
		local uuid7 = servers.generate_uuid7()
		local hash256 = servers.sha256("feature-lab")
		local hash128 = servers.sha128(buffer.fromstring("feature-lab"))
		assert(decoded.nested.ok == true)
		assert(#uuid4 > 20 and #uuid7 > 20)
		assert(#hash256 == 64 and #hash128 == 32)
		return string.format("roundtrip ok uuid=%s hash=%s", uuid4:sub(1, 8), hash256:sub(1, 8))
	end)
end

probe("modules + aliases", function()
	assert(userInput == input)
	assert(command == commands)
	assert(prefab == prefabs)
	assert(tween == tweening)
	assert(transforms == transform)
	assert(core.Image2D ~= nil and core.Sprite2D ~= nil)
	assert(core.TextLabel == core.TextBox)
	assert(core.RudimentaryTextLabel == core.TextBox)
	assert(core.String2D == core.Rope2D)
	assert(core["9SliceSprite2D"] == core.NineSliceSprite2D)
	assert(inlineModule.marker == "inline softrequire")
	return "require, softrequire, and aliases"
end)

runFsProbe()
runServerHelpers()

featureLabAsyncTask = async(function()
	async.yield()
	assert(featureLabAsyncTask:getStatus() == "running")
	setResult("async", true, "queued, yielded, and resumed across updates")
	return "async complete"
end)
assert(async.count() >= 1)

local shaderHandles = {}
probe("shaders", function()
	fs.writeFile("runtime/lab_vertex.glsl", shaders.DEFAULT_VERTEX_SHADER)
	shaderHandles[1] = shaders.loadFragment("lab_shader.glsl", {})
	shaderHandles[2] = shaders.load("runtime/lab_vertex.glsl", "lab_shader.glsl", {})
	shaderHandles[3] = shaders.fromFragmentSource(fs.readFile("lab_shader.glsl"), {})
	shaderHandles[4] = shaders.fromSource(shaders.DEFAULT_VERTEX_SHADER, fs.readFile("lab_shader.glsl"), {})
	for _, shader in ipairs(shaderHandles) do
		shader:setUniform1f("u_time", 0)
		shader:setUniform2f("u_resolution", window.x, window.y)
		shader:setUniform3f("u_tint3", 0.7, 0.9, 1.0)
		shader:setUniform4f("u_tint4", 1.0, 0.8, 0.7, 1.0)
		shader:setUniformColor("u_color", Color4(255, 255, 255, 255))
		shader:setTexture("u_extra", generatedImage)
	end
	return "load/loadFragment/fromSource/fromFragmentSource"
end)

local titleEntity, titleLabel = makeLabel(
	"title",
	ecs.root,
	24,
	18,
	900,
	46,
	"NEOLOVE / COMPLETE FEATURE LAB",
	34,
	WHITE,
	"left",
	"center"
)
titleLabel.letter_spacing = 1.5

local subtitleEntity, subtitleLabel = makeLabel(
	"subtitle",
	ecs.root,
	24,
	60,
	900,
	28,
	"Live engine telemetry, deterministic smoke checks, and opt-in external probes",
	17,
	MUTED,
	"left",
	"center"
)

local renderPanel = makePanel("render_panel", 24, 104, 430, 236, PANEL)
local physicsPanel = makePanel("physics_panel", 470, 104, 430, 236, PANEL_ALT)
local runtimePanel = makePanel("runtime_panel", 24, 356, 430, 220, PANEL_ALT)
local statusPanel = makePanel("status_panel", 470, 356, 430, 220, PANEL)

makeLabel("render_title", renderPanel, 12, 8, 406, 28, "01 / RENDER + ASSETS", 21, CYAN, "left", "center")
makeLabel("physics_title", physicsPanel, 12, 8, 406, 28, "02 / PHYSICS + TRANSFORMS", 21, AMBER, "left", "center")
makeLabel("runtime_title", runtimePanel, 12, 8, 406, 28, "03 / INPUT + RUNTIME", 21, GREEN, "left", "center")
makeLabel("status_title", statusPanel, 12, 8, 406, 28, "04 / PROBE LEDGER", 21, BLUE, "left", "center")

local shapeNames = { "box", "circle", "right_triangle" }
local shapeColors = { BLUE, GREEN, AMBER }
for index, shapeName in ipairs(shapeNames) do
	local shapeEntity = makeEntity("shape_" .. shapeName, renderPanel, 18 + (index - 1) * 76, 54, 54, 54, 2)
	shapeEntity.rotation_pivot = "middle"
	local shape = addShape(shapeEntity, shapeName, shapeColors[index])
	if shapeName == "right_triangle" then
		shape.triangle_corner = "br"
	end
end

local spriteEntity = makeEntity("sprite", renderPanel, 252, 50, 64, 64, 3)
local sprite = spriteEntity:AddComponent(core.Sprite2D)
sprite.image = generatedImage
sprite.color = WHITE

local croppedEntity = makeEntity("cropped_sprite", renderPanel, 330, 50, 64, 64, 3)
local croppedSprite = croppedEntity:AddComponent(core.Image2D)
croppedSprite.image = generatedImage
croppedSprite.source_x = 12
croppedSprite.source_y = 12
croppedSprite.source_w = 24
croppedSprite.source_h = 24
croppedSprite.color = Color4(255, 220, 150, 255)

local tileEntity = makeEntity("tile_texture", renderPanel, 18, 126, 194, 72, 1)
local tile = tileEntity:AddComponent(core.TileTexture2D)
tile.image = generatedImage
tile.tile_width = 28
tile.tile_height = 28
tile.offset_x = 0
tile.offset_y = 0
tile.color = Color4(130, 190, 255, 255)

local nineEntity = makeEntity("nine_slice", renderPanel, 228, 126, 184, 72, 1)
local nine = nineEntity:AddComponent(core.NineSliceSprite2D)
nine.image = generatedImage
nine.slice_left = 14
nine.slice_right = 14
nine.slice_top = 14
nine.slice_bottom = 14
nine.color = Color4(255, 255, 255, 210)

local spritebox = spriteEntity:AddComponent(core.Spritebox2D)
spritebox.alpha_threshold = 32
spritebox:ComputeSpritebox()
probe("spritebox", function()
	local inside = spritebox:IsInside(32, 32)
	local intersects = spritebox:isIntersecting(croppedEntity)
	return string.format("rects=%d inside=%s intersects=%s", spritebox.rect_count, tostring(inside), tostring(intersects))
end)

local shaderEntity = makeEntity("shader_strip", renderPanel, 18, 208, 394, 8, 1)
local shaderRect = addRect(shaderEntity, WHITE)
if shaderHandles[1] then
	shaderRect.shader = shaderHandles[1]
end

local floor = makeEntity("floor", physicsPanel, 16, 186, 398, 20, 0)
addRect(floor, Color4(72, 84, 108, 255))
local floorCollider = floor:AddComponent(core.Collider2D)
floorCollider.shape = "box"
floorCollider.friction = 0.9
floorCollider.restitution = 0.25

local anchor = makeEntity("rope_anchor", physicsPanel, 42, 58, 22, 22, 3)
addShape(anchor, "circle", CYAN)
local anchorBody = anchor:AddComponent(core.Rigidbody2D)
anchorBody.is_static = true
local anchorCollider = anchor:AddComponent(core.Collider2D)
anchorCollider.shape = "circle"

local body = makeEntity("physics_body", physicsPanel, 178, 58, 40, 40, 4)
body.rotation_pivot = "middle"
addShape(body, "circle", AMBER)
local rigidbody = body:AddComponent(core.Rigidbody2D)
rigidbody.bounds_mode = "window"
rigidbody.mass = 1.4
rigidbody.gravity_y = 520
rigidbody.gravity_scale = 1
rigidbody.linear_damping = 0.08
rigidbody.angular_damping = 0.14
rigidbody.restitution = 0.7
rigidbody.friction = 0.4
rigidbody.sleep_epsilon = 0.001
rigidbody.max_speed = 900
rigidbody.max_angular_speed = 16
rigidbody.collision_enabled = true
rigidbody:setVelocity(35, 0)
rigidbody:setAngularVelocity(0.8)
rigidbody:setGravity(0, 520)
rigidbody:addForce(1, 0)
rigidbody:addImpulse(1, 0)
rigidbody:addTorque(0.1)
rigidbody:addAngularImpulse(0.1)
local physicsCollider = body:AddComponent(core.Collider2D)
physicsCollider.shape = "circle"
physicsCollider.friction = 0.4
physicsCollider.restitution = 0.7
physicsCollider.enabled = true
physicsCollider.non_physics = false

local trigger = makeEntity("trigger", physicsPanel, 286, 120, 88, 52, 2)
addRect(trigger, Color4(76, 135, 255, 70))
local triggerCollider = trigger:AddComponent(core.Collider2D)
triggerCollider.is_trigger = true
triggerCollider.shape = "box"

local ropeEntity = makeEntity("rope", physicsPanel, 0, 0, 1, 1, 1)
local rope = ropeEntity:AddComponent(core.String2D)
rope:link(anchor, body)
rope.min_length = 60
rope.max_length = 150
rope.stiffness = 0.86
rope.damping = 0.18
rope.break_force = 9000

local ropeLine = makeEntity("rope_line", physicsPanel, 0, 0, 1, 3, 2)
addRect(ropeLine, Color4(190, 215, 232, 220))

physicsCollider:setOnCollisionEnter(function()
	counters.collisionEnter += 1
end)
physicsCollider:setOnCollisionStay(function()
	counters.collisionStay += 1
end)
physicsCollider:setOnCollisionExit(function()
	counters.collisionExit += 1
end)
triggerCollider:setOnTriggerEnter(function()
	counters.triggerEnter += 1
end)
triggerCollider:setOnTriggerStay(function()
	counters.triggerStay += 1
end)
triggerCollider:setOnTriggerExit(function()
	counters.triggerExit += 1
end)

local eventPad = makeEntity("event_pad", runtimePanel, 18, 48, 166, 72, 5)
local eventPadShape = addRect(eventPad, Color4(48, 72, 108, 255))
local eventPadText = eventPad:AddComponent(core.TextLabel)
eventPadText.text = "CLICK / SCROLL"
eventPadText.scale = 18
eventPadText.color = WHITE
eventPadText.size_mode = "entity"
eventPadText.align_x = "center"
eventPadText.align_y = "center"

local eventCounts = { left = 0, right = 0, middle = 0, up = 0, down = 0 }
eventPad:Listen("leftClick", function()
	eventCounts.left += 1
	eventPadShape.color = Color4(52, 110, 146, 255)
end)
eventPad:Listen("rightClick", function()
	eventCounts.right += 1
	eventPadShape.color = Color4(112, 72, 142, 255)
end)
eventPad:Listen("middleClick", function()
	eventCounts.middle += 1
	eventPadShape.color = Color4(132, 92, 52, 255)
end)
eventPad:Listen("scrollUp", function()
	eventCounts.up += 1
	eventPad.rotation += 0.08
end)
eventPad:Listen("scrollDown", function()
	eventCounts.down += 1
	eventPad.rotation -= 0.08
end)

local secondaryConnection
local function connectSecondaryListener()
	secondaryConnection = eventPad:listen("leftClick", function()
		counters.echo += 1
	end)
end
connectSecondaryListener()

local lifecycleComponent = {
	name = "LabLifecycle",
	awake = function(entity, component)
		counters.awake += 1
		component.frames = 0
		assert(component:GetEntity() == entity)
	end,
	update = function(_entity, component)
		counters.update += 1
		component.frames += 1
	end,
	destroy = function()
		counters.destroy += 1
	end,
	onDestroy = function()
		counters.destroy += 1
	end,
}

local lifecycleEntity = makeEntity("lifecycle_entity", runtimePanel, 204, 48, 92, 32, 2)
addRect(lifecycleEntity, Color4(45, 130, 91, 255))
local lifecycleInstance = lifecycleEntity:AddComponent(lifecycleComponent)
assert(lifecycleInstance:getEntity() == lifecycleEntity)

local removeEntity = makeEntity("remove_test", runtimePanel, 310, 48, 92, 32, 2)
local removeInstance = removeEntity:addComponent(lifecycleComponent)
removeEntity:RemoveComponent(removeInstance)
removeEntity:AddComponent(lifecycleComponent):Remove()

local child = makeEntity("child", lifecycleEntity, 12, 8, 22, 16, 4)
child.anchor_x = 0.1
child.anchor_y = 0.1
child.pivot_x = 1
child.pivot_y = 1
child.position_pivot = "center"
addRect(child, CYAN)
local duplicate = child:Duplicate(lifecycleEntity)
duplicate.name = "child_copy"
duplicate.x = 50
local directDuplicate = ecs.duplicateEntity(child, lifecycleEntity)
directDuplicate.name = "child_direct_copy"
directDuplicate.x = 74
assert(lifecycleEntity:FindFirstChild("child") == child)
assert(ecs.findFirstChild(lifecycleEntity, "child_copy") == duplicate)
local worldX, worldY = child:GetWorldPosition()
local worldRotation = child:getWorldRotation()
probe("ecs + hierarchy", function()
	assert(worldX ~= nil and worldY ~= nil and worldRotation ~= nil)
	assert(transform.getWorldPosition(child) ~= nil)
	assert(transforms.getWorldRotation(child) ~= nil)
	return "methods, lifecycle, duplicate, find, world transform"
end)

local temporary = ecs.newEntity("temporary", runtimePanel, 0, 0)
ecs.deleteEntity(temporary)
local temporaryMethod = ecs.newEntity("temporary_method", runtimePanel, 0, 0)
temporaryMethod:delete()
local directRemoveEntity = ecs.newEntity("direct_remove", runtimePanel, 0, 0)
local directRemoveComponent = ecs.addComponent(directRemoveEntity, lifecycleComponent)
ecs.removeComponent(directRemoveEntity, directRemoveComponent)

local prefabSource = makeEntity("prefab_source", runtimePanel, 204, 92, 92, 28, 2)
addRect(prefabSource, Color4(107, 72, 160, 255))
local captured = prefabs.capture(prefabSource)
local prefabComponent = prefabs.component(core.Shape2D, {
	shape = "circle",
	color = GREEN,
})
local registered = prefabs.register("feature_lab_chip", {
	name = "feature_lab_chip",
	x = 0,
	y = 0,
	size_x = 24,
	size_y = 24,
	components = { prefabComponent },
})
assert(prefab.get("feature_lab_chip") == registered)
local prefabInstance = prefabs.instantiate("feature_lab_chip", runtimePanel)
prefabInstance.x = 318
prefabInstance.y = 94
local prefabCopy = prefab.duplicate(captured, runtimePanel)
prefabCopy.x = 354
prefabCopy.y = 94
prefabs.remove("feature_lab_chip")

probe("prefabs", function()
	local names = { "label", "panel", "dialog", "statusChip", "status_chip" }
	for _, name in ipairs(names) do
		assert(prefabs.ui[name] ~= nil)
	end
	return "capture/register/get/remove/instantiate/duplicate/ui"
end)

local tweenDot = makeEntity("tween_dot", runtimePanel, 18, 144, 22, 22, 3)
addShape(tweenDot, "circle", GREEN)
local tweenHandles = {}
local function launchTweens()
	tweening.cancel_all()
	tweenDot.x = 18
	tweenDot.scale = 1
	tweenDot.rotation = 0
	tweenHandles[1] = tweening.to(tweenDot, "x", 360, 1.6, "sine", "inOut")
	tweenHandles[2] = tween.new(tweenDot, "rotation", math.pi * 2, 1.6, "quad", "out")
	tweenHandles[3] = tween.create(tweenDot, "scale", 1.8, 0.8, "back", "out", function()
		tweening.to(tweenDot, "scale", 1, 0.6, "bounce", "out")
	end)
end
launchTweens()
probe("tweening", function()
	local value = tweening.ease(0.5, "sine", "inOut")
	assert(value > 0 and tweening.count() >= 1)
	return "to/new/create/ease/count/cancel aliases"
end)

local telemetryEntity, telemetryLabel = makeLabel(
	"telemetry",
	runtimePanel,
	18,
	174,
	384,
	34,
	"",
	14,
	WHITE,
	"left",
	"center"
)
telemetryLabel.text_scale = "fit_width"
telemetryLabel.wrap = "none"

local ledgerEntity, ledgerLabel = makeLabel(
	"ledger",
	statusPanel,
	12,
	40,
	406,
	102,
	"",
	14,
	WHITE,
	"left",
	"top"
)
ledgerLabel.text_scale = "fit"
ledgerLabel.line_spacing = 1.05

local logEntity, logLabel = makeLabel(
	"log",
	statusPanel,
	12,
	146,
	406,
	62,
	"",
	13,
	MUTED,
	"left",
	"top"
)
logLabel.text_scale = "fit"
logLabel.line_spacing = 1.05

local hostedServer
local serverClient
local fpsVisible = true
local loopVolume = 0.35
local elapsed = 0

local function runHttpGet()
	probe("http.get queued", function()
		return http.get("https://example.com/", function(response)
			setResult("http.get callback", response.ok, string.format("status=%s bytes=%d", tostring(response.status), #(response.body or "")))
		end)
	end)
end

local function runHttpRequest()
	probe("http.request queued", function()
		return http.request({
			url = "https://example.com/",
			method = "GET",
			headers = { ["X-NeoLOVE-Sample"] = "feature-lab" },
		}, function(response)
			setResult("http.request callback", response.ok, response.error or ("status=" .. tostring(response.status)))
		end)
	end)
end

local function runCommand()
	probe("commands.run", function()
		local output = command.run("printf", { "NeoLOVE command API" }, ".")
		assert(output.ok)
		return string.format("status=%d stdout=%s", output.status_code, output.stdout)
	end)
end

local function runDetachedCommand()
	probe("commands.detached", function()
		local output = commands.runDetached("true", {}, ".")
		assert(output.ok)
		return "pid=" .. tostring(output.pid)
	end)
end

local function runNetworkProbe()
	if hostedServer then
		setResult("local server", true, "already running at " .. hostedServer:getUrl())
		return
	end
	probe("local server", function()
		hostedServer = servers.host("echo_server.luau", 0, { host = "127.0.0.1" })
		assert(hostedServer:getPort() > 0)
		assert(hostedServer.port == hostedServer:getPort())
		assert(hostedServer.url == hostedServer:getUrl())

		local hostClient = hostedServer.client
		assert(hostClient:isHost())
		assert(hostClient.is_host == true)
		assert(hostClient:getKey() == hostClient.key)
		assert(hostClient:isConnected())
		assert(hostClient:getKickReason() == nil)

		serverClient = servers.connect(hostedServer.url)
		assert(not serverClient:isHost())
		serverClient:addcallback(function(payload)
			local decoded = servers.deserializeTable(payload)
			counters.echo += 1
			setResult("server echo", decoded.message == "hello", decoded.message)
			serverClient:disconnect()
			hostedServer:stop()
			hostedServer = nil
			serverClient = nil
		end)
		local sent = serverClient:send(servers.serialize_table({ message = "hello" }))
		assert(sent)
		return "host/connect/send/callback/disconnect/stop"
	end)
end

local function resetBody()
	body.x = 178
	body.y = 58
	body.rotation = 0
	rigidbody:setVelocity(35, 0)
	rigidbody:setAngularVelocity(0.8)
end

local system = {
	awake = function()
		counters.systemAwake += 1
	end,
	update = function(_self, dt)
		counters.systemUpdate += 1
		elapsed += dt

		local dx, dy = input.getMouseDelta()
		local wheelX, wheelY = userInput.getMouseWheel()
		local lastKey = input.getLastKeyPressed() or "-"
		local char = input.getCharPressed() or "-"
		local scroll = input.getScrollInAmount()
		local locked = input.isMouseLocked()
		local mouseFlags = string.format(
			"%s/%s/%s",
			tostring(input.isMouseDown("left")),
			tostring(input.isMousePressed("left")),
			tostring(input.isMouseReleased("left"))
		)
		telemetryLabel.text = string.format(
			"mouse %.0f,%.0f d%.0f,%.0f btn=%s wheel=%.0f,%.0f in/out=%s/%s amount=%.1f key=%s char=%s lock=%s",
			mouse.x,
			mouse.y,
			dx,
			dy,
			mouseFlags,
			wheelX,
			wheelY,
			tostring(input.isScrollingIn()),
			tostring(input.isScrollingOut()),
			scroll,
			lastKey,
			char,
			tostring(locked)
		)

		if input.isKeyPressed("1") then
			audio.playOnce(generatedSound, 0.7)
		end
		if input.isKeyPressed("2") then
			audio.play(generatedSound, true, loopVolume)
		end
		if input.isKeyPressed("3") then
			audio.stop(generatedSound)
		end
		if input.isKeyPressed("4") then
			loopVolume = loopVolume == 0.35 and 0.75 or 0.35
			audio.setVolume(generatedSound, loopVolume)
		end
		if input.isKeyPressed("t") then
			for _, handle in ipairs(tweenHandles) do
				if not handle:IsDone() then
					handle:Cancel()
				end
			end
			launchTweens()
		end
		if input.isKeyPressed("g") then
			runAssetRoundTrip()
		end
		if input.isKeyPressed("f") then
			runFsProbe()
		end
		if input.isKeyPressed("h") then
			runHttpGet()
		end
		if input.isKeyPressed("j") then
			runHttpRequest()
		end
		if input.isKeyPressed("c") then
			runCommand()
		end
		if input.isKeyPressed("d") then
			runDetachedCommand()
		end
		if input.isKeyPressed("n") then
			runNetworkProbe()
		end
		if input.isKeyPressed("m") then
			input.setMouseLocked(not input.isMouseLocked())
		end
		if input.isKeyPressed("p") then
			fpsVisible = not fpsVisible
			app.setShowFps(fpsVisible)
		end
		if input.isKeyPressed("r") then
			resetBody()
		end
		if input.isKeyPressed("k") then
			if secondaryConnection and secondaryConnection:IsConnected() then
				secondaryConnection:Disconnect()
				setResult("event connection", true, "secondary disconnected")
			else
				connectSecondaryListener()
				setResult("event connection", secondaryConnection:isConnected(), "secondary connected")
			end
		end
		if input.isKeyPressed("delete") then
			die("feature lab intentional die() test")
		end

		if input.isKeyDown("left") then
			rigidbody:addForce(-850 * dt, 0)
		end
		if input.isKeyDown("right") then
			rigidbody:addForce(850 * dt, 0)
		end
		if input.isKeyPressed("up") then
			rigidbody:addImpulse(0, -220)
		end
		if input.isKeyPressed("down") then
			rigidbody:addTorque(8)
		end

		local anchorX, anchorY = anchor:GetWorldPosition()
		local bodyX, bodyY = body:GetWorldPosition()
		local deltaX = bodyX + body.size_x * 0.5 - (anchorX + anchor.size_x * 0.5)
		local deltaY = bodyY + body.size_y * 0.5 - (anchorY + anchor.size_y * 0.5)
		local distance = math.sqrt(deltaX * deltaX + deltaY * deltaY)
		ropeLine.x = anchor.x + anchor.size_x * 0.5
		ropeLine.y = anchor.y + anchor.size_y * 0.5
		ropeLine.size_x = distance
		ropeLine.rotation = math.atan2(deltaY, deltaX)

		local hit = transform.raycast(bodyX, bodyY, 1, 0, 220, { ignoreEntity = body })
		local overlapping = transforms.doTheyOverlap({ body, trigger })
		local velocityX, velocityY = rigidbody:getVelocity()
		local angular = rigidbody:getAngularVelocity()
		results.physics = {
			ok = true,
			detail = string.format(
				"v=%.0f,%.0f w=%.1f ray=%s overlap=%s rope=%.0f/%.0f",
				velocityX,
				velocityY,
				angular,
				hit and hit.entity.name or "-",
				tostring(overlapping),
				rope.current_length,
				rope.tension
			),
		}

		for _, shader in ipairs(shaderHandles) do
			shader:setUniform1f("u_time", elapsed)
			shader:setUniform2f("u_resolution", window.x, window.y)
		end
		tile.offset_x = elapsed * 12
		tile.offset_y = elapsed * 7
		spriteEntity.rotation = elapsed * 0.35
		croppedEntity.rotation = -elapsed * 0.25
	end,
	lateUpdate = function()
		counters.systemLate += 1

		local names = {}
		for name in pairs(results) do
			table.insert(names, name)
		end
		table.sort(names)
		local lines = {}
		for index, name in ipairs(names) do
			if index > 6 then
				break
			end
			local result = results[name]
			table.insert(lines, string.format("[%s] %s: %s", result.ok and "PASS" or "FAIL", name, result.detail))
		end
		ledgerLabel.text = table.concat(lines, "\n")
		logLabel.text = table.concat(logs, "\n")

		titleEntity.size_x = math.max(300, window.x - 48)
		subtitleEntity.size_x = math.max(300, window.x - 48)

		local panelWidth = math.max(320, (window.x - 72) * 0.5)
		local leftX = 24
		local rightX = 48 + panelWidth
		renderPanel.x = leftX
		renderPanel.size_x = panelWidth
		physicsPanel.x = rightX
		physicsPanel.size_x = panelWidth
		runtimePanel.x = leftX
		runtimePanel.size_x = panelWidth
		statusPanel.x = rightX
		statusPanel.size_x = panelWidth
	end,
	fixedUpdate = function()
		counters.systemFixed += 1
	end,
}

ecs.addSystem(system)

probe("application", function()
	assert(app.getMaxFps() == 120)
	assert(app.getShowFps() == true)
	assert(app.getNearestNeighborScaling() == true)
	return string.format("window=%.0fx%.0f fps=120 nearest=true", window.x, window.y)
end)

probe("render components", function()
	assert(core.Rect2D and core.Shape2D and core.TextBox)
	assert(core.Sprite2D and core.NineSliceSprite2D and core.TileTexture2D)
	return "rect/shape/text/sprite/nine-slice/tile"
end)

setResult("external probes", true, "H/J HTTP, C/D commands, N server, Delete die")
setResult("audio controls", true, "1 once, 2 loop, 3 stop, 4 volume")
setResult("input controls", true, "mouse pad, arrows, M lock, P FPS")

addLog("Feature lab ready")
