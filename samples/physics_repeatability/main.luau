app.bg = Color4(10, 12, 20, 255)
app.setMaxFps(240)

local TRIAL_COUNT = 6
local FRAMES_PER_TRIAL = 420
local COOLDOWN_FRAMES = 45

local trialIndex = 0
local frameInTrial = 0
local cooldownLeft = 0
local baselineHash = nil
local runDone = false
local activeEntities = {}
local bodies = {}
local runResults = {}

local function cloneColor(c)
	return Color4(c.r, c.g, c.b, c.a)
end

local function q(value)
	return string.format("%.5f", value or 0)
end

local function hashString(s)
	local h = 5381
	for i = 1, #s do
		h = (h * 33 + string.byte(s, i)) % 4294967291
	end
	return h
end

local function rememberEntity(entity)
	activeEntities[#activeEntities + 1] = entity
	return entity
end

local function spawnBody(spec)
	local e = rememberEntity(ecs.newEntity(spec.name, ecs.root, spec.x, spec.y))
	e.size_x = spec.w
	e.size_y = spec.h
	e.rotation = spec.rotation or 0
	e.rotation_pivot = "middle"

	local shape = ecs.addComponent(e, core.Shape2D)
	shape.color = cloneColor(spec.color or Color4(230, 230, 230, 255))
	shape.shape = spec.shape or "box"
	if shape.shape == "right_triangle" then
		shape.triangle_corner = spec.corner or "bl"
	end

	local collider = ecs.addComponent(e, core.Collider2D)
	collider.shape = spec.shape or "box"
	if collider.shape == "right_triangle" then
		collider.triangle_corner = spec.corner or "bl"
	end
	collider.friction = spec.friction or 0.5
	collider.restitution = spec.restitution or 0.2

	local rb = nil
	if spec.dynamic or spec.static_body then
		rb = ecs.addComponent(e, core.Rigidbody2D)
		rb.is_static = spec.static_body == true
		rb.mass = spec.mass or 1.0
		rb.friction = spec.friction or 0.5
		rb.restitution = spec.restitution or 0.2
		rb.gravity_y = spec.gravityY or 920
		rb.linear_damping = spec.linear_damping or 0.05
		rb.angular_damping = spec.angular_damping or 0.08
		rb.max_speed = 3000
		rb.max_angular_speed = 80
		if spec.vx or spec.vy then
			rb:setVelocity(spec.vx or 0, spec.vy or 0)
		end
		if spec.angular_velocity then
			rb:setAngularVelocity(spec.angular_velocity)
		end
	end

	local body = {
		entity = e,
		collider = collider,
		rb = rb,
	}
	bodies[#bodies + 1] = body
	return body
end

local function addRope(bodyA, bodyB, maxLength, stiffness, damping)
	local ropeEntity = rememberEntity(ecs.newEntity("rope", ecs.root, 0, 0))
	ropeEntity.size_x = 1
	ropeEntity.size_y = 1
	ropeEntity.z = -1

	local rope = ecs.addComponent(ropeEntity, core.Rope2D)
	rope:link(bodyA.entity, bodyB.entity)
	rope.max_length = maxLength
	rope.stiffness = stiffness
	rope.damping = damping
	rope.break_force = 0
	return rope
end

local function resetScene()
	for i = #activeEntities, 1, -1 do
		ecs.deleteEntity(activeEntities[i])
		table.remove(activeEntities, i)
	end
	bodies = {}

	local ww = window.x
	local wh = window.y
	local cx = ww * 0.5
	local top = wh * 0.16

	spawnBody({
		name = "floor",
		x = cx - 380,
		y = wh - 56,
		w = 760,
		h = 40,
		rotation = 0,
		shape = "box",
		color = Color4(54, 64, 88, 255),
		static_body = true,
		friction = 0.9,
		restitution = 0.08,
	})
	spawnBody({
		name = "left_wall",
		x = 24,
		y = 44,
		w = 26,
		h = wh - 100,
		rotation = 0,
		shape = "box",
		color = Color4(40, 52, 74, 255),
		static_body = true,
		friction = 0.9,
		restitution = 0.1,
	})
	spawnBody({
		name = "right_wall",
		x = ww - 50,
		y = 44,
		w = 26,
		h = wh - 100,
		rotation = 0,
		shape = "box",
		color = Color4(40, 52, 74, 255),
		static_body = true,
		friction = 0.9,
		restitution = 0.1,
	})
	spawnBody({
		name = "ramp_l",
		x = cx - 320,
		y = wh - 220,
		w = 180,
		h = 120,
		shape = "right_triangle",
		corner = "bl",
		color = Color4(64, 78, 108, 255),
		static_body = true,
		friction = 0.92,
		restitution = 0.05,
	})
	spawnBody({
		name = "ramp_r",
		x = cx + 140,
		y = wh - 220,
		w = 180,
		h = 120,
		shape = "right_triangle",
		corner = "br",
		color = Color4(64, 78, 108, 255),
		static_body = true,
		friction = 0.92,
		restitution = 0.05,
	})

	local dynamicSpecs = {
		{ x = cx - 140, y = top + 0, w = 30, h = 30, shape = "circle", mass = 0.7, vx = 90, vy = 0, angular_velocity = 3.2, color = Color4(245, 217, 132, 255) },
		{ x = cx - 80, y = top + 0, w = 34, h = 24, shape = "box", mass = 0.9, vx = -70, vy = 0, angular_velocity = -2.2, rotation = 0.2, color = Color4(168, 211, 250, 255) },
		{ x = cx - 20, y = top + 0, w = 36, h = 36, shape = "right_triangle", corner = "tr", mass = 1.0, vx = 40, vy = 0, angular_velocity = 1.7, rotation = -0.6, color = Color4(248, 171, 171, 255) },
		{ x = cx + 40, y = top + 0, w = 28, h = 28, shape = "circle", mass = 0.6, vx = -30, vy = 0, angular_velocity = 4.0, color = Color4(194, 239, 164, 255) },
		{ x = cx + 100, y = top + 0, w = 42, h = 20, shape = "box", mass = 1.1, vx = 20, vy = 0, angular_velocity = -1.3, rotation = 0.5, color = Color4(210, 179, 245, 255) },
		{ x = cx + 170, y = top + 0, w = 34, h = 34, shape = "right_triangle", corner = "bl", mass = 0.95, vx = -55, vy = 0, angular_velocity = 2.8, rotation = 0.1, color = Color4(255, 195, 136, 255) },
		{ x = cx - 170, y = top - 70, w = 26, h = 44, shape = "box", mass = 1.2, vx = 0, vy = 0, angular_velocity = -0.8, rotation = 0.25, color = Color4(138, 220, 224, 255) },
		{ x = cx + 200, y = top - 60, w = 38, h = 26, shape = "box", mass = 0.8, vx = 0, vy = 0, angular_velocity = 1.9, rotation = -0.35, color = Color4(252, 168, 210, 255) },
	}

	local dynamicBodies = {}
	for i = 1, #dynamicSpecs do
		local spec = dynamicSpecs[i]
		spec.name = "dyn_" .. tostring(i)
		spec.dynamic = true
		spec.gravityY = 920
		spec.friction = 0.42
		spec.restitution = 0.32
		spec.linear_damping = 0.04
		spec.angular_damping = 0.07
		dynamicBodies[#dynamicBodies + 1] = spawnBody(spec)
	end

	addRope(dynamicBodies[1], dynamicBodies[2], 74, 0.9, 0.18)
	addRope(dynamicBodies[3], dynamicBodies[4], 78, 0.9, 0.18)
	addRope(dynamicBodies[5], dynamicBodies[6], 88, 0.92, 0.2)
end

local function buildSnapshot()
	local parts = {}
	for i = 1, #bodies do
		local b = bodies[i]
		local e = b.entity
		local rb = b.rb
		local touching = b.collider.touching and "1" or "0"
		local vx = rb and rb.velocity_x or 0
		local vy = rb and rb.velocity_y or 0
		local av = rb and rb.angular_velocity or 0
		local row = table.concat({
			tostring(i),
			q(e.x),
			q(e.y),
			q(e.rotation),
			q(vx),
			q(vy),
			q(av),
			touching,
		}, "|")
		parts[#parts + 1] = row
	end
	return table.concat(parts, ";")
end

local function startTrial(index)
	trialIndex = index
	frameInTrial = 0
	cooldownLeft = 0
	resetScene()
end

local function completeTrial()
	local snapshot = buildSnapshot()
	local hash = hashString(snapshot)
	local sameAsBaseline = false

	if baselineHash == nil then
		baselineHash = hash
		sameAsBaseline = true
	else
		sameAsBaseline = hash == baselineHash
	end

	runResults[#runResults + 1] = {
		index = trialIndex,
		hash = hash,
		same = sameAsBaseline,
	}

	if trialIndex >= TRIAL_COUNT then
		runDone = true
	else
		cooldownLeft = COOLDOWN_FRAMES
	end
end

local function restartSuite()
	baselineHash = nil
	runResults = {}
	runDone = false
	startTrial(1)
end

local title = ecs.newEntity("title", ecs.root, 16, 30)
local titleLabel = ecs.addComponent(title, core.TextBox)
titleLabel.scale = 28
titleLabel.color = Color4(225, 235, 255, 255)
titleLabel.text = "Physics Repeatability Test"

local info = ecs.newEntity("info", ecs.root, 16, 56)
local infoLabel = ecs.addComponent(info, core.TextBox)
infoLabel.scale = 18
infoLabel.color = Color4(162, 182, 224, 255)
infoLabel.text = "R: restart tests. Each trial uses exact same object sizes, positions, and rotations."

local status = ecs.newEntity("status", ecs.root, 16, 82)
local statusLabel = ecs.addComponent(status, core.TextBox)
statusLabel.scale = 18
statusLabel.color = Color4(178, 205, 255, 255)

local resultLine = ecs.newEntity("resultline", ecs.root, 16, 108)
local resultLineLabel = ecs.addComponent(resultLine, core.TextBox)
resultLineLabel.scale = 18
resultLineLabel.color = Color4(216, 226, 255, 255)

local history = ecs.newEntity("history", ecs.root, 16, 134)
local historyLabel = ecs.addComponent(history, core.TextBox)
historyLabel.scale = 18
historyLabel.color = Color4(144, 168, 216, 255)
historyLabel.text = ""

restartSuite()

ecs.addSystem({
	update = function(_self, dt)
		if input.isKeyPressed("r") then
			restartSuite()
		end

		if not runDone then
			if cooldownLeft > 0 then
				cooldownLeft -= 1
				if cooldownLeft <= 0 then
					startTrial(trialIndex + 1)
				end
			else
				frameInTrial += 1
				if frameInTrial >= FRAMES_PER_TRIAL then
					completeTrial()
				end
			end
		end

		local passCount = 0
		local hashLines = {}
		for i = 1, #runResults do
			local r = runResults[i]
			if r.same then
				passCount += 1
			end
			hashLines[#hashLines + 1] = string.format(
				"Run %d: %u %s",
				r.index,
				r.hash,
				r.same and "MATCH" or "DIFFERENT"
			)
		end

		local state = runDone and "Done" or (cooldownLeft > 0 and "Cooldown" or "Running")
		statusLabel.text = string.format(
			"State: %s  Trial: %d/%d  Frame: %d/%d",
			state,
			math.max(1, trialIndex),
			TRIAL_COUNT,
			math.min(frameInTrial, FRAMES_PER_TRIAL),
			FRAMES_PER_TRIAL
		)

		if baselineHash then
			resultLineLabel.text = string.format(
				"Baseline hash: %u  Matches: %d/%d",
				baselineHash,
				passCount,
				#runResults
			)
		else
			resultLineLabel.text = "Baseline hash: (pending)"
		end

		historyLabel.text = #hashLines > 0 and table.concat(hashLines, "   ") or ""
	end,
})
