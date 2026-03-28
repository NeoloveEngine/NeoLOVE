app.bg = Color4(255, 255, 255, 255)
app.setMaxFps(144)

math.randomseed(os.time())

local function clamp(v, lo, hi)
	if v < lo then
		return lo
	end
	if v > hi then
		return hi
	end
	return v
end

local function approach(current, target, speed)
	return current + (target - current) * clamp(speed, 0, 1)
end

local function project(worldX, z, worldY, focal, centerX, horizonY, roadDepth)
	local depth = focal / (z + focal)
	local x = centerX + worldX * depth
	local y = horizonY + depth * roadDepth - worldY * depth
	return x, y, depth
end

local shaderFiles = {}
for i = 0, 24 do
	shaderFiles[#shaderFiles + 1] = string.format("shaders/warp_%02d.glsl", i)
end

local shaderNames = {
	"Spectral Tide", "Warp Mist", "Pulse Spiral", "Glass Bloom", "Ion Drift",
	"Solar Veil", "Prism Wave", "Fractal Glow", "Echo Rings", "Lumen Flux",
	"Auric Drift", "Velvet Plasma", "Nova Rain", "Hex Mirage", "Phase Field",
	"Twist Rift", "Celestial Warp", "Night Neon", "Static Bloom", "Quantum Ripples",
	"Gilded Storm", "Dream Current", "Prismatic Noise", "Cosmic Ribbon", "Signal Abyss",
}

local fxShaderOptions = {
	uniforms = {
		{ name = "u_time", type = "float1" },
		{ name = "u_resolution", type = "float2" },
		{ name = "u_intensity", type = "float1" },
	}
}

local roadShaderOptions = {
	uniforms = {
		{ name = "u_time", type = "float1" },
		{ name = "u_resolution", type = "float2" },
		{ name = "u_intensity", type = "float1" },
		{ name = "u_scroll", type = "float1" },
		{ name = "u_center_near", type = "float1" },
		{ name = "u_center_far", type = "float1" },
		{ name = "u_width_near", type = "float1" },
		{ name = "u_width_far", type = "float1" },
		{ name = "u_wave", type = "float1" },
	}
}

local shadersFx = {}
for _, path in ipairs(shaderFiles) do
	shadersFx[#shadersFx + 1] = shaders.loadFragment(path, fxShaderOptions)
end

local roadShader = shaders.loadFragment("shaders/road.glsl", roadShaderOptions)

local playScale = 0.9
local activeShader = 1
local pendingShader = 1
local transitionT = 1.0
local isTransitioning = false

local shaderIntensity = 0.22
local boostTimer = 0
local boostCooldown = 0
local hitFlash = 0
local worldTime = 0
local distance = 0
local shaderMilestone = 0

local score = 0
local lives = 5
local energy = 55
local gameOver = false

local lane = 0
local laneTarget = 0

local bgA = ecs.newEntity("bg_a", ecs.root, 0, 0)
bgA.z = -60
local bgARenderer = ecs.addComponent(bgA, core.Rect2D)
bgARenderer.color = Color4(255, 255, 255, 255)
bgARenderer.shader = shadersFx[activeShader]

local bgB = ecs.newEntity("bg_b", ecs.root, 0, 0)
bgB.z = -59
local bgBRenderer = ecs.addComponent(bgB, core.Rect2D)
bgBRenderer.color = Color4(255, 255, 255, 0)
bgBRenderer.shader = shadersFx[activeShader]

local road = ecs.newEntity("road", ecs.root, 0, 0)
road.z = -20
local roadRenderer = ecs.addComponent(road, core.Rect2D)
roadRenderer.color = Color4(255, 255, 255, 255)
roadRenderer.shader = roadShader

local horizon = ecs.newEntity("horizon", ecs.root, 0, 0)
horizon.z = -18
local horizonRenderer = ecs.addComponent(horizon, core.Rect2D)
horizonRenderer.color = Color4(232, 240, 251, 255)

local stars = {}
for i = 1, 90 do
	local e = ecs.newEntity("star_" .. tostring(i), ecs.root, 0, 0)
	e.z = -40
	e.size_x = 2
	e.size_y = 2
	local r = ecs.addComponent(e, core.Rect2D)
	r.color = Color4(90, 96, 112, 170)
	stars[#stars + 1] = {
		entity = e,
		renderer = r,
		x = math.random(-1100, 1100),
		y = math.random(120, 460),
		z = math.random(120, 2200),
	}
end

local player = ecs.newEntity("player", ecs.root, 0, 0)
player.z = 90
player.size_x = 22
player.size_y = 22
local playerRenderer = ecs.addComponent(player, core.Rect2D)
playerRenderer.color = Color4(22, 28, 42, 255)

local playerGlow = ecs.newEntity("player_glow", ecs.root, 0, 0)
playerGlow.z = 89
playerGlow.size_x = 28
playerGlow.size_y = 28
local playerGlowRenderer = ecs.addComponent(playerGlow, core.Rect2D)
playerGlowRenderer.color = Color4(58, 135, 232, 140)

local title = ecs.newEntity("title", ecs.root, 16, 30)
title.z = 120
local titleLabel = ecs.addComponent(title, core.TextBox)
titleLabel.scale = 30
titleLabel.color = Color4(18, 25, 36, 255)

local hint = ecs.newEntity("hint", ecs.root, 16, 58)
hint.z = 120
local hintLabel = ecs.addComponent(hint, core.TextBox)
hintLabel.scale = 18
hintLabel.color = Color4(68, 78, 96, 255)
hintLabel.text = "Move: Left/Right or A/D  Warp: Space  Shader: Q/E or 0-9  Scale: [ / ]"

local hud = ecs.newEntity("hud", ecs.root, 16, 84)
hud.z = 120
local hudLabel = ecs.addComponent(hud, core.TextBox)
hudLabel.scale = 18
hudLabel.color = Color4(74, 88, 108, 255)

local status = ecs.newEntity("status", ecs.root, 16, 108)
status.z = 120
local statusLabel = ecs.addComponent(status, core.TextBox)
statusLabel.scale = 21
statusLabel.color = Color4(152, 36, 44, 255)
statusLabel.text = ""

local objects = {}
local SPAWN_Z_MIN = 1280
local SPAWN_Z_MAX = 2450
local SPAWN_LANES = { -1, 0, 1 }

local function removeObject(index)
	ecs.deleteEntity(objects[index].entity)
	table.remove(objects, index)
end

local function beginShaderTransition(index)
	if index < 1 then
		index = #shadersFx
	end
	if index > #shadersFx then
		index = 1
	end
	if index == activeShader then
		return
	end

	pendingShader = index
	bgBRenderer.shader = shadersFx[pendingShader]
	bgBRenderer.color = Color4(255, 255, 255, 0)
	transitionT = 0
	isTransitioning = true
end

local function updateShaderTransition(dt)
	if not isTransitioning then
		bgARenderer.color = Color4(255, 255, 255, 255)
		bgBRenderer.color = Color4(255, 255, 255, 0)
		return
	end

	transitionT = clamp(transitionT + dt * 1.85, 0, 1)
	local a = math.floor((1 - transitionT) * 255)
	local b = math.floor(transitionT * 255)
	bgARenderer.color = Color4(255, 255, 255, a)
	bgBRenderer.color = Color4(255, 255, 255, b)

	if transitionT >= 1 then
		activeShader = pendingShader
		isTransitioning = false
		bgA, bgB = bgB, bgA
		bgARenderer, bgBRenderer = bgBRenderer, bgARenderer
		bgBRenderer.color = Color4(255, 255, 255, 0)
	end
end

local function chooseSpawnLaneAndZ(kind)
	local minGap = kind == "hazard" and 250 or 210
	for _ = 1, 36 do
		local lane = SPAWN_LANES[math.random(1, #SPAWN_LANES)]
		local z = math.random(SPAWN_Z_MIN, SPAWN_Z_MAX)
		local ok = true
		for i = 1, #objects do
			local o = objects[i]
			if o.lane == lane and math.abs(o.z - z) < minGap then
				ok = false
				break
			end
		end
		if ok then
			return lane, z
		end
	end
	return SPAWN_LANES[math.random(1, #SPAWN_LANES)], SPAWN_Z_MAX - math.random(0, 240)
end

local function spawnObject(kind)
	local e = ecs.newEntity("obj_" .. tostring(math.random(1000000)), ecs.root, 0, 0)
	e.z = 30
	e.size_x = 20
	e.size_y = 20

	local r = ecs.addComponent(e, core.Rect2D)
	if kind == "orb" then
		r.color = Color4(242, 182, 52, 255)
	else
		r.color = Color4(214, 64, 72, 255)
	end

	local lanePick, zPick = chooseSpawnLaneAndZ(kind)
	objects[#objects + 1] = {
		entity = e,
		renderer = r,
		kind = kind,
		lane = lanePick,
		z = zPick,
		wobble = math.random() * 6.28318,
		nearMissed = false,
	}
end

local function resetGame()
	score = 0
	lives = 5
	energy = 55
	lane = 0
	laneTarget = 0
	gameOver = false
	statusLabel.text = ""
	distance = 0
	worldTime = 0
	boostTimer = 0
	boostCooldown = 0
	hitFlash = 0
	shaderIntensity = 0.22
	shaderMilestone = 0
	transitionT = 1
	isTransitioning = false

	for i = #objects, 1, -1 do
		ecs.deleteEntity(objects[i].entity)
		table.remove(objects, i)
	end

	for _ = 1, 4 do
		spawnObject("hazard")
	end
	for _ = 1, 3 do
		spawnObject("orb")
	end
end

resetGame()
local spawnTimer = 0

ecs.addSystem({
	update = function(_self, dt)
		if dt > 0.1 then
			dt = 0.1
		end

		if input.isKeyDown("rightbracket") then
			playScale += dt * 0.9
		end
		if input.isKeyDown("leftbracket") then
			playScale -= dt * 0.9
		end
		playScale = clamp(playScale, 0.45, 1.0)

		if input.isKeyPressed("q") then
			beginShaderTransition(activeShader - 1)
		end
		if input.isKeyPressed("e") then
			beginShaderTransition(activeShader + 1)
		end
		for digit = 0, 9 do
			if input.isKeyPressed(tostring(digit)) then
				beginShaderTransition(digit + 1)
			end
		end

		if gameOver and input.isKeyPressed("enter") then
			resetGame()
		end

		worldTime += dt

		local fxW = window.x * playScale
		local fxH = window.y * playScale
		local fxX = (window.x - fxW) * 0.5
		local fxY = (window.y - fxH) * 0.56
		bgA.x = fxX
		bgA.y = fxY
		bgA.size_x = fxW
		bgA.size_y = fxH
		bgB.x = fxX
		bgB.y = fxY
		bgB.size_x = fxW
		bgB.size_y = fxH
		road.x = fxX
		road.y = fxY
		road.size_x = fxW
		road.size_y = fxH

		local centerX = fxX + fxW * 0.5
		local horizonY = fxY + fxH * 0.18
		local roadDepth = fxH * 0.84
		local laneGap = fxW * 0.41

		horizon.x = fxX
		horizon.y = horizonY - 8
		horizon.size_x = fxW
		horizon.size_y = 16

		local baseSpeed = 305
		local speed = baseSpeed + score * 0.15
		speed = clamp(speed, 305, 780)

		if boostCooldown > 0 then
			boostCooldown -= dt
		end
		if hitFlash > 0 then
			hitFlash -= dt
		end

		if not gameOver then
			if input.isKeyPressed("left") or input.isKeyPressed("a") then
				laneTarget -= 1
			end
			if input.isKeyPressed("right") or input.isKeyPressed("d") then
				laneTarget += 1
			end
			laneTarget = clamp(laneTarget, -1, 1)

			if input.isKeyPressed("space") and energy >= 22 and boostCooldown <= 0 then
				energy -= 22
				boostTimer = 1.15
				boostCooldown = 0.68
				beginShaderTransition(activeShader + 1)
			end
		end

		if boostTimer > 0 then
			boostTimer -= dt
		end
		local boost = clamp(boostTimer / 1.15, 0, 1)

		lane = approach(lane, laneTarget, dt * (9 + boost * 8))

		local velocity = speed * (1 + boost * 0.72)
		if not gameOver then
			distance += velocity * dt
			score += velocity * dt * 0.06
			energy = clamp(energy + dt * 6.0, 0, 100)
		end

		local milestone = math.floor(distance / 980)
		if milestone > shaderMilestone then
			shaderMilestone = milestone
			beginShaderTransition(activeShader + 1)
		end

		shaderIntensity = 0.2 + boost * 0.95 + hitFlash * 0.75 + clamp((speed - baseSpeed) / 460, 0, 1) * 0.4
		shaderIntensity = clamp(shaderIntensity, 0.14, 1.8)

		local curveNow = (
			math.sin(worldTime * 0.55) * 0.72 +
			math.sin(worldTime * 0.17 + 1.1) * 0.42
		) * (0.20 + shaderIntensity * 0.18)

		local focal = 245 - shaderIntensity * 28
		focal = clamp(focal, 175, 275)

		updateShaderTransition(dt)

		local function curveOffsetAtZ(z)
			local farZ = 2400
			local k = clamp(z / farZ, 0.0, 1.0)
			return curveNow * fxW * 0.18 * (k * k)
		end

		local roadCenterNear = clamp((centerX + curveOffsetAtZ(44) - fxX) / fxW, 0.06, 0.94)
		local roadCenterFar = clamp((centerX + curveOffsetAtZ(2140) - fxX) / fxW, 0.06, 0.94)
		local roadWidthNear = clamp(0.44 + shaderIntensity * 0.02, 0.40, 0.48)
		local roadWidthFar = clamp(0.016 + shaderIntensity * 0.006, 0.012, 0.028)
		local roadWave = clamp(0.006 + shaderIntensity * 0.004, 0.004, 0.015)

		for i = 1, #stars do
			local s = stars[i]
			s.z -= velocity * dt * (0.23 + boost * 0.92)
			if s.z < 42 then
				s.z = math.random(1400, 2600)
				s.x = math.random(-1200, 1200)
				s.y = math.random(130, 500)
			end

			local sx, sy, sd = project(s.x, s.z, s.y, focal, centerX + curveOffsetAtZ(s.z), horizonY, roadDepth)
			s.entity.x = sx
			s.entity.y = sy
			local sz = clamp(sd * 7.2, 1.0, 4.2)
			s.entity.size_x = sz
			s.entity.size_y = sz
			s.entity.z = -45 + sd * 12
			local c = clamp(84 + sd * 190, 70, 255)
			s.renderer.color = Color4(c, c, c + 10, 150)
		end

		local playerZ = 44
		local playerWorldX = lane * laneGap * 0.48
		local px, py, pDepth = project(playerWorldX, playerZ, -8, focal, centerX + curveOffsetAtZ(playerZ), horizonY, roadDepth)
		local pSize = clamp(104 * pDepth, 28, 92)
		player.size_x = pSize
		player.size_y = pSize
		player.x = px - pSize * 0.5
		player.y = py - pSize * 0.66
		player.z = 96
		player.rotation = -lane * 0.15 + math.sin(worldTime * 7.5) * 0.05 * boost
		player.rotation_pivot = "middle"

		playerGlow.size_x = pSize * (1.22 + boost * 0.52)
		playerGlow.size_y = pSize * (1.22 + boost * 0.52)
		playerGlow.x = px - playerGlow.size_x * 0.5
		playerGlow.y = py - playerGlow.size_y * 0.6
		playerGlow.z = 95
		playerGlow.rotation = -player.rotation * 0.7
		playerGlow.rotation_pivot = "middle"
		if boost > 0 then
			playerGlowRenderer.color = Color4(255, 126, 64, 168)
		else
			playerGlowRenderer.color = Color4(58, 135, 232, 140)
		end

		spawnTimer -= dt
		if not gameOver and spawnTimer <= 0 then
			spawnTimer = clamp(0.64 - score * 0.0002, 0.22, 0.64)
			local maxObjects = clamp(9 + math.floor(score / 250), 9, 17)
			if #objects < maxObjects then
				local hazards = 0
				local orbs = 0
				for i = 1, #objects do
					if objects[i].kind == "hazard" then
						hazards += 1
					else
						orbs += 1
					end
				end
				if orbs < 3 and math.random() < 0.38 then
					spawnObject("orb")
				elseif hazards < 10 then
					spawnObject("hazard")
				else
					spawnObject("orb")
				end
			end
		end

		for i = #objects, 1, -1 do
			local o = objects[i]
			o.z -= velocity * dt

			local laneX = o.lane * laneGap * 0.48
			laneX += math.sin(worldTime * 2.3 + o.wobble + o.z * 0.01) * fxW * 0.013 * shaderIntensity
			local ox, oy, od = project(laneX, o.z, 0, focal, centerX + curveOffsetAtZ(o.z), horizonY, roadDepth)

			local baseSize = o.kind == "hazard" and 112 or 86
			local size = clamp(baseSize * od, 7, 118)
			o.entity.size_x = size
			o.entity.size_y = size
			o.entity.x = ox - size * 0.5
			o.entity.y = oy - size * 0.60
			o.entity.z = 18 + od * 80
			o.entity.rotation = math.sin(worldTime * 1.6 + o.wobble) * 0.11
			o.entity.rotation_pivot = "middle"

			if o.kind == "hazard" then
				local tint = clamp(120 + od * 130, 90, 255)
				o.renderer.color = Color4(tint, 45, 52, 235)
			else
				local blink = 0.5 + 0.5 * math.sin(worldTime * 5.6 + o.wobble)
				local tint = clamp(150 + blink * 90 + od * 80, 120, 255)
				o.renderer.color = Color4(tint, 176, 52, 245)
			end

			if o.z < 26 then
				removeObject(i)
			elseif not gameOver then
				local laneDelta = math.abs(o.lane - lane)
				if o.kind == "hazard" then
					if not o.nearMissed and o.z < 102 and laneDelta < 0.52 then
						o.nearMissed = true
						score += 8 + boost * 9
						energy = clamp(energy + 2, 0, 100)
						shaderIntensity = clamp(shaderIntensity + 0.08, 0.0, 1.8)
					end
					if o.z < 74 and laneDelta < 0.34 then
						lives -= 1
						hitFlash = 1.0
						energy = clamp(energy - 24, 0, 100)
						boostTimer = math.max(boostTimer, 0.36)
						removeObject(i)
					end
				else
					if o.z < 76 and laneDelta < 0.38 then
						score += 14
						energy = clamp(energy + 14, 0, 100)
						shaderIntensity = clamp(shaderIntensity + 0.12, 0.0, 1.8)
						removeObject(i)
					end
				end
			end
		end

		if not gameOver and lives <= 0 then
			gameOver = true
			statusLabel.text = "Core Collapse - Press Enter to Restart"
		end

		shadersFx[activeShader]:setUniform1f("u_time", worldTime)
		shadersFx[activeShader]:setUniform2f("u_resolution", fxW, fxH)
		shadersFx[activeShader]:setUniform1f("u_intensity", shaderIntensity)
		if isTransitioning then
			shadersFx[pendingShader]:setUniform1f("u_time", worldTime)
			shadersFx[pendingShader]:setUniform2f("u_resolution", fxW, fxH)
			shadersFx[pendingShader]:setUniform1f("u_intensity", shaderIntensity)
		end

		roadShader:setUniform1f("u_time", worldTime)
		roadShader:setUniform2f("u_resolution", fxW, fxH)
		roadShader:setUniform1f("u_intensity", shaderIntensity)
		roadShader:setUniform1f("u_scroll", distance * 0.017)
		roadShader:setUniform1f("u_center_near", roadCenterNear)
		roadShader:setUniform1f("u_center_far", roadCenterFar)
		roadShader:setUniform1f("u_width_near", roadWidthNear)
		roadShader:setUniform1f("u_width_far", roadWidthFar)
		roadShader:setUniform1f("u_wave", roadWave)

		titleLabel.text = string.format(
			"Warp Rush 3D  Score: %d  Lives: %d  Energy: %d",
			math.floor(score),
			lives,
			math.floor(energy)
		)
		hudLabel.text = string.format(
			"Shader %02d/24: %s  Intensity %.2f  Scale %.2f  Dist %dm",
			activeShader - 1,
			shaderNames[activeShader],
			shaderIntensity,
			playScale,
			math.floor(distance * 0.01)
		)
	end,
})
