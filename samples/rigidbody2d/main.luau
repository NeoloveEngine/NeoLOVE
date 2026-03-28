app.bg = Color4(13, 16, 24, 255)
app.setMaxFps(144)

math.randomseed(os.time())

local bodies = {}
local ropes = {}
local bodyId = 0
local ropeId = 0
local selectedShape = "box"

local function randomRange(minValue, maxValue)
	return minValue + math.random() * (maxValue - minValue)
end

local function makeColor(r, g, b)
	return Color4(math.clamp(r, 0, 255), math.clamp(g, 0, 255), math.clamp(b, 0, 255), 255)
end

local function cloneColor(c)
	return Color4(c.r, c.g, c.b, c.a)
end

local function bodyCenter(entity)
	return entity.x + entity.size_x * 0.5, entity.y + entity.size_y * 0.5
end

local function spawnDynamicBody(opts)
	bodyId += 1
	local e = ecs.newEntity("body_" .. tostring(bodyId), ecs.root, opts.x, opts.y)
	e.size_x = opts.w
	e.size_y = opts.h
	e.rotation_pivot = "middle"

	local renderer = ecs.addComponent(e, core.Shape2D)
	renderer.color = cloneColor(opts.color)
	renderer.shape = opts.shape
	if opts.shape == "right_triangle" then
		renderer.triangle_corner = opts.triangleCorner or "bl"
	end

	local rb = ecs.addComponent(e, core.Rigidbody2D)
	rb.bounds_mode = "window"
	rb.mass = opts.mass
	rb.restitution = opts.restitution
	rb.friction = opts.friction
	rb.linear_damping = opts.linearDamping or 0.14
	rb.angular_damping = opts.angularDamping or 0.22
	rb.gravity_y = opts.gravityY or 920
	rb.max_speed = 1800
	rb.max_angular_speed = 26

	local collider = ecs.addComponent(e, core.Collider2D)
	collider.shape = opts.shape
	if opts.shape == "right_triangle" then
		collider.triangle_corner = opts.triangleCorner or "bl"
	end
	collider.friction = opts.friction
	collider.restitution = opts.restitution

	local body = {
		entity = e,
		renderer = renderer,
		rb = rb,
		collider = collider,
		baseColor = cloneColor(opts.color),
		shape = opts.shape,
	}
	bodies[#bodies + 1] = body
	return body
end

local function spawnStaticRamp(x, y, w, h, corner)
	local e = ecs.newEntity("ramp_" .. tostring(math.random(1000000)), ecs.root, x, y)
	e.size_x = w
	e.size_y = h
	e.rotation_pivot = "middle"
	e.z = -1

	local renderer = ecs.addComponent(e, core.Shape2D)
	renderer.color = Color4(68, 78, 102, 255)
	renderer.shape = "right_triangle"
	renderer.triangle_corner = corner

	local collider = ecs.addComponent(e, core.Collider2D)
	collider.shape = "right_triangle"
	collider.triangle_corner = corner
	collider.friction = 0.9
	collider.restitution = 0.1
	collider.enabled = true

	return {
		entity = e,
		renderer = renderer,
		collider = collider,
	}
end

local function addRope(bodyA, bodyB, maxLength, stiffness)
	ropeId += 1
	local ropeEntity = ecs.newEntity("rope_" .. tostring(ropeId), ecs.root, 0, 0)
	ropeEntity.size_x = 1
	ropeEntity.size_y = 1
	ropeEntity.z = -2

	local rope = ecs.addComponent(ropeEntity, core.Rope2D)
	rope:link(bodyA.entity, bodyB.entity)
	rope.max_length = maxLength
	rope.stiffness = stiffness or 0.88
	rope.damping = 0.22
	rope.break_force = 12000

	local line = ecs.newEntity("rope_line_" .. tostring(ropeId), ecs.root, 0, 0)
	line.size_x = maxLength
	line.size_y = 3
	line.z = -1
	local lineRenderer = ecs.addComponent(line, core.Rect2D)
	lineRenderer.color = Color4(212, 220, 234, 210)

	local obj = {
		ropeEntity = ropeEntity,
		rope = rope,
		line = line,
		lineRenderer = lineRenderer,
		bodyA = bodyA,
		bodyB = bodyB,
	}
	ropes[#ropes + 1] = obj
	return obj
end

local function spawnSelected(x, y)
	if selectedShape == "box" then
		local w = randomRange(18, 38)
		local h = randomRange(16, 34)
		local b = spawnDynamicBody({
			x = x - w * 0.5,
			y = y - h * 0.5,
			w = w,
			h = h,
			shape = "box",
			mass = math.max(0.45, (w * h) * 0.0016),
			restitution = randomRange(0.15, 0.45),
			friction = randomRange(0.35, 0.68),
			color = makeColor(randomRange(95, 245), randomRange(95, 230), randomRange(120, 235)),
		})
		b.rb:setVelocity(randomRange(-120, 120), randomRange(-220, -80))
	elseif selectedShape == "circle" then
		local d = randomRange(16, 36)
		local b = spawnDynamicBody({
			x = x - d * 0.5,
			y = y - d * 0.5,
			w = d,
			h = d,
			shape = "circle",
			mass = math.max(0.4, d * 0.05),
			restitution = randomRange(0.4, 0.92),
			friction = randomRange(0.05, 0.32),
			color = makeColor(randomRange(120, 245), randomRange(140, 250), randomRange(95, 220)),
		})
		b.rb:setVelocity(randomRange(-150, 150), randomRange(-200, -70))
	else
		local w = randomRange(22, 44)
		local h = randomRange(22, 44)
		local corners = { "bl", "br", "tl", "tr" }
		local b = spawnDynamicBody({
			x = x - w * 0.5,
			y = y - h * 0.5,
			w = w,
			h = h,
			shape = "right_triangle",
			triangleCorner = corners[math.random(1, #corners)],
			mass = math.max(0.5, (w * h) * 0.0015),
			restitution = randomRange(0.18, 0.55),
			friction = randomRange(0.2, 0.55),
			color = makeColor(randomRange(180, 250), randomRange(110, 220), randomRange(110, 230)),
		})
		b.rb:setVelocity(randomRange(-130, 130), randomRange(-210, -80))
		b.rb:addAngularImpulse(randomRange(-8, 8))
	end
end

local function spawnRopeBridge(x, y)
	local chain = {}
	for i = 1, 8 do
		local d = 18
		local body = spawnDynamicBody({
			x = x + (i - 1) * 26,
			y = y,
			w = d,
			h = d,
			shape = "circle",
			mass = 0.9,
			restitution = 0.2,
			friction = 0.6,
			color = Color4(205, 220, 248, 255),
			linearDamping = 0.35,
		})
		if i == 1 or i == 8 then
			body.rb.is_static = true
		end
		chain[#chain + 1] = body
	end
	for i = 1, #chain - 1 do
		addRope(chain[i], chain[i + 1], 30, 0.92)
	end
end

local title = ecs.newEntity("title", ecs.root, 16, 30)
local titleLabel = ecs.addComponent(title, core.TextBox)
titleLabel.scale = 30
titleLabel.color = Color4(220, 230, 255, 255)
titleLabel.text = "Rigidbody2D + Collider2D + Rope2D"

local hint = ecs.newEntity("hint", ecs.root, 16, 58)
local hintLabel = ecs.addComponent(hint, core.TextBox)
hintLabel.scale = 18
hintLabel.color = Color4(166, 184, 224, 255)
hintLabel.text = "Select shape: 1 box, 2 circle, 3 right-triangle. Spawn: left click. Blast: right click"

local hint2 = ecs.newEntity("hint2", ecs.root, 16, 80)
local hint2Label = ecs.addComponent(hint2, core.TextBox)
hint2Label.scale = 18
hint2Label.color = Color4(150, 172, 216, 255)
hint2Label.text = "R: spawn rope bridge  C: clear dynamic  Arrow keys: impulse on newest body"

local stats = ecs.newEntity("stats", ecs.root, 16, 104)
local statsLabel = ecs.addComponent(stats, core.TextBox)
statsLabel.scale = 17
statsLabel.color = Color4(130, 153, 200, 255)

spawnStaticRamp(80, 340, 180, 110, "bl")
spawnStaticRamp(math.max(260, window.x - 280), 320, 180, 120, "br")
spawnRopeBridge(220, 110)

for _ = 1, 24 do
	spawnSelected(randomRange(120, math.max(150, window.x - 140)), randomRange(0, window.y * 0.3))
end

ecs.addSystem({
	update = function(_self, dt)
		if input.isKeyPressed("1") then
			selectedShape = "box"
		end
		if input.isKeyPressed("2") then
			selectedShape = "circle"
		end
		if input.isKeyPressed("3") then
			selectedShape = "triangle"
		end

		if input.isMousePressed("left") then
			spawnSelected(mouse.x, mouse.y)
		end

		if input.isMousePressed("right") then
			for i = 1, #bodies do
				local body = bodies[i]
				if not body.rb.is_static then
					local cx, cy = bodyCenter(body.entity)
					local dx = cx - mouse.x
					local dy = cy - mouse.y
					local distSq = dx * dx + dy * dy
					if distSq > 1 and distSq < 260 * 260 then
						local dist = math.sqrt(distSq)
						local strength = (1 - dist / 260) * 1200
						body.rb:addImpulse((dx / dist) * strength, (dy / dist) * strength)
						body.rb:addAngularImpulse(randomRange(-16, 16) * (1 - dist / 260))
					end
				end
			end
		end

		if input.isKeyPressed("r") then
			spawnRopeBridge(randomRange(120, math.max(140, window.x - 340)), randomRange(60, window.y * 0.32))
		end

		if input.isKeyPressed("c") then
			for i = #bodies, 1, -1 do
				if not bodies[i].rb.is_static then
					ecs.deleteEntity(bodies[i].entity)
					table.remove(bodies, i)
				end
			end
		end

		if #bodies > 0 then
			local latest = bodies[#bodies]
			local impulseX = 0
			local impulseY = 0
			if input.isKeyPressed("left") then
				impulseX -= 260
			end
			if input.isKeyPressed("right") then
				impulseX += 260
			end
			if input.isKeyPressed("up") then
				impulseY -= 320
			end
			if input.isKeyPressed("down") then
				impulseY += 320
			end
			if impulseX ~= 0 or impulseY ~= 0 then
				latest.rb:addImpulse(impulseX, impulseY)
			end
		end

		local touchingCount = 0
		for i = 1, #bodies do
			local body = bodies[i]
			if body.collider.touching then
				touchingCount += 1
				body.renderer.color = Color4(255, 245, 150, 255)
			else
				body.renderer.color = cloneColor(body.baseColor)
			end
		end

		for i = #ropes, 1, -1 do
			local ropeObj = ropes[i]
			local a = ropeObj.bodyA
			local b = ropeObj.bodyB
			if ropeObj.rope.enabled and a.entity and b.entity then
				local ax, ay = bodyCenter(a.entity)
				local bx, by = bodyCenter(b.entity)
				local dx = bx - ax
				local dy = by - ay
				local dist = math.sqrt(dx * dx + dy * dy)
				ropeObj.line.x = ax
				ropeObj.line.y = ay
				ropeObj.line.size_x = dist
				ropeObj.line.size_y = 3
				ropeObj.line.rotation = math.atan2(dy, dx)
				ropeObj.lineRenderer.color = Color4(212, 220, 234, 210)
			else
				ropeObj.line.size_x = 0
				ropeObj.lineRenderer.color = Color4(255, 110, 110, 120)
			end
		end

		local fps = dt > 0 and math.floor(1 / dt + 0.5) or 0
		statsLabel.text = string.format(
			"Bodies: %d  Touching: %d  Ropes: %d  Shape: %s  FPS: %d",
			#bodies,
			touchingCount,
			#ropes,
			selectedShape,
			fps
		)
	end,
})
