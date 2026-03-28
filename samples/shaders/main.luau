app.bg = Color4(255, 255, 255, 255)
app.setMaxFps(120)

local shaderFiles = {
	"shader0_pulse.glsl",
	"shader1_fluid.glsl",
	"shader2_plasma.glsl",
	"shader3_voronoi.glsl",
	"shader4_tunnel.glsl",
	"shader5_ocean.glsl",
	"shader6_fire.glsl",
	"shader7_lava.glsl",
	"shader8_kaleido.glsl",
	"shader9_glitch.glsl",
	"shader10_aurora.glsl",
	"shader11_marble.glsl",
	"shader12_starfield.glsl",
	"shader13_hexwave.glsl",
	"shader14_moire.glsl",
}

local shaderNames = {
	"Pulse Grid",
	"Fluid Flow",
	"Neon Plasma",
	"Voronoi Cells",
	"Warp Tunnel",
	"Ocean Bands",
	"Fire Column",
	"Lava Veins",
	"Kaleidoscope",
	"Glitch Scan",
	"Aurora Veil",
	"Marble Flow",
	"Starfield Warp",
	"Hex Wave",
	"Moire Rings",
}

local shaderOptions = {
	uniforms = {
		{ name = "u_time", type = "float1" },
		{ name = "u_resolution", type = "float2" },
	}
}

local shadersLoaded = {}
for _, path in ipairs(shaderFiles) do
	shadersLoaded[#shadersLoaded + 1] = shaders.loadFragment(path, shaderOptions)
end

local box = ecs.newEntity("shader_box", ecs.root, 0, 0)
local baseW = 360
local baseH = 240
local scale = 1.0
local scaleSpeed = 0.9

local renderer = ecs.addComponent(box, core.Rect2D)
renderer.color = Color4(255, 255, 255, 255)

local title = ecs.newEntity("title", ecs.root, 20, 32)
local titleLabel = ecs.addComponent(title, core.TextBox)
titleLabel.scale = 28
titleLabel.color = Color4(20, 30, 45, 255)

local hint = ecs.newEntity("hint", ecs.root, 20, 62)
local hintLabel = ecs.addComponent(hint, core.TextBox)
hintLabel.scale = 20
hintLabel.color = Color4(60, 75, 100, 255)
hintLabel.text = "0-9 core shaders, Left/Right all shaders, Up increase scale, Down decrease scale"

local perf = ecs.newEntity("perf", ecs.root, 20, 88)
local perfLabel = ecs.addComponent(perf, core.TextBox)
perfLabel.scale = 18
perfLabel.color = Color4(70, 86, 110, 255)
perfLabel.text = "Scale controls: Up increase, Down decrease"

local currentShaderIndex = 1
renderer.shader = shadersLoaded[currentShaderIndex]

local function setShaderIndex(index)
	if index < 1 then
		index = #shadersLoaded
	end
	if index > #shadersLoaded then
		index = 1
	end

	currentShaderIndex = index
	renderer.shader = shadersLoaded[currentShaderIndex]
	titleLabel.text = "Shader " .. tostring(currentShaderIndex - 1) .. ": " .. shaderNames[currentShaderIndex]
end

setShaderIndex(1)

local t = 0
ecs.addSystem({
	update = function(_self, dt)
		t += dt

		for digit = 0, 9 do
			if input.isKeyPressed(tostring(digit)) then
				setShaderIndex(digit + 1)
			end
		end

		if input.isKeyPressed("right") then
			setShaderIndex(currentShaderIndex + 1)
		end
		if input.isKeyPressed("left") then
			setShaderIndex(currentShaderIndex - 1)
		end

		if input.isKeyDown("up") then
			scale += dt * scaleSpeed
		end
		if input.isKeyDown("down") then
			scale -= dt * scaleSpeed
		end
		scale = math.clamp(scale, 0.25, 3.8)

		local w = baseW * scale
		local h = baseH * scale
		box.size_x = w
		box.size_y = h
		box.x = (window.x - w) * 0.5
		box.y = (window.y - h) * 0.56

		local activeShader = shadersLoaded[currentShaderIndex]
		activeShader:setUniform1f("u_time", t)
		activeShader:setUniform2f("u_resolution", w, h)

		local pixels = math.floor(w * h)
		perfLabel.text = "Scale: " .. string.format("%.2f", scale) .. "  Surface: " .. tostring(pixels) .. " px"
	end
})
