app.bg = Color4(12, 16, 24, 255)
app.setMaxFps(120)

local TRACK_LEFT = 110
local TRACK_WIDTH = 520
local TRACK_TOP = 140
local TRACK_GAP = 78
local DOT_SIZE = 34

local styles = {
	{ name = "linear", style = "linear", direction = "out", color = Color4(125, 211, 252, 255) },
	{ name = "sine inOut", style = "sine", direction = "inOut", color = Color4(167, 139, 250, 255) },
	{ name = "quad out", style = "quad", direction = "out", color = Color4(74, 222, 128, 255) },
	{ name = "back out", style = "back", direction = "out", color = Color4(251, 191, 36, 255) },
	{ name = "bounce out", style = "bounce", direction = "out", color = Color4(251, 113, 133, 255) },
}

local dots = {}
local goingRight = true

local function rect(name, x, y, w, h, z, color)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	entity.size_x = w
	entity.size_y = h
	entity.z = z
	local renderer = ecs.addComponent(entity, core.Rect2D)
	renderer.color = color
	return entity, renderer
end

local function circle(name, x, y, size, z, color)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	entity.size_x = size
	entity.size_y = size
	entity.z = z
	local renderer = ecs.addComponent(entity, core.Shape2D)
	renderer.shape = "circle"
	renderer.color = color
	return entity, renderer
end

local function label(name, x, y, text, scale, color)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	entity.size_x = 420
	entity.size_y = 34
	entity.z = 10
	local renderer = ecs.addComponent(entity, core.TextBox)
	renderer.text = text
	renderer.scale = scale
	renderer.color = color
	return entity, renderer
end

label("title", 40, 42, "Tweening", 34, Color4(241, 245, 249, 255))
label("hint", 40, 82, "Press Space to reverse every tween. Press R to restart.", 20, Color4(148, 163, 184, 255))

for index, config in ipairs(styles) do
	local y = TRACK_TOP + (index - 1) * TRACK_GAP
	rect("track_" .. config.style, TRACK_LEFT, y + 13, TRACK_WIDTH, 8, 0, Color4(38, 48, 66, 255))
	label("label_" .. config.style, 40, y + 2, config.name, 18, Color4(203, 213, 225, 255))
	local dot = circle("dot_" .. config.style, TRACK_LEFT, y, DOT_SIZE, 5, config.color)
	dots[#dots + 1] = {
		entity = dot,
		config = config,
	}
end

local function launchTweens()
	local targetX = goingRight and (TRACK_LEFT + TRACK_WIDTH - DOT_SIZE) or TRACK_LEFT
	for _, dot in ipairs(dots) do
		tweening.to(dot.entity, "x", targetX, 1.45, dot.config.style, dot.config.direction)
	end
	goingRight = not goingRight
end

launchTweens()

ecs.addSystem({
	update = function()
		if input.isKeyPressed("space") then
			tweening.cancelAll()
			launchTweens()
		end

		if input.isKeyPressed("r") then
			tweening.cancelAll()
			goingRight = true
			for _, dot in ipairs(dots) do
				dot.entity.x = TRACK_LEFT
			end
			launchTweens()
		end
	end,
})
