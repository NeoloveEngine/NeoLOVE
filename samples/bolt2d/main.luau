app.bg = Color4(11, 13, 17, 255)
app.setMaxFps(144)

local target = nil
local targetPivot = nil
local bolts = {}
local targetX = window.x * 0.5
local targetY = window.y * 0.38
local targetRotation = 0

local function colorCopy(color)
	return Color4(color.r, color.g, color.b, color.a)
end

local function makeLabel(name, x, y, size, color, text)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	local label = ecs.addComponent(entity, core.TextBox)
	label.scale = size
	label.color = color
	label.text = text
	return label
end

local function makeShape(name, x, y, w, h, shape, color, z)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	entity.size_x = w
	entity.size_y = h
	entity.rotation_pivot = "middle"
	entity.z = z or 0

	local draw = ecs.addComponent(entity, core.Shape2D)
	draw.shape = shape
	draw.color = colorCopy(color)

	return entity, draw
end

local function makeLine(name, color, z)
	local entity = ecs.newEntity(name, ecs.root, 0, 0)
	entity.size_x = 1
	entity.size_y = 3
	entity.z = z or -1

	local draw = ecs.addComponent(entity, core.Rect2D)
	draw.color = color

	return entity, draw
end

local function makePhysicsBody(name, x, y, w, h, shape, color, isStatic)
	local entity, draw = makeShape(name, x, y, w, h, shape, color, 0)

	local rb = ecs.addComponent(entity, core.Rigidbody2D)
	rb.is_static = isStatic or false
	rb.bounds_mode = "window"
	rb.gravity_y = 360
	rb.mass = math.max(0.5, (w * h) * 0.0012)
	rb.linear_damping = 0.16
	rb.angular_damping = 0.75
	rb.restitution = 0.18
	rb.friction = 0.65
	rb.max_speed = 1600
	rb.max_angular_speed = 14

	local collider = ecs.addComponent(entity, core.Collider2D)
	collider.shape = shape
	collider.friction = rb.friction
	collider.restitution = rb.restitution

	return {
		entity = entity,
		draw = draw,
		rb = rb,
		collider = collider,
		baseColor = colorCopy(color),
	}
end

local function bodyPivot(entity)
	return entity.x + entity.size_x * 0.5, entity.y + entity.size_y * 0.5
end

local function rotatePoint(x, y, rotation)
	local cosR = math.cos(rotation)
	local sinR = math.sin(rotation)
	return x * cosR - y * sinR, x * sinR + y * cosR
end

local function targetOffsetPoint(offsetX, offsetY)
	local rx, ry = rotatePoint(offsetX, offsetY, target.entity.rotation or 0)
	local px, py = bodyPivot(target.entity)
	return px + rx, py + ry
end

local function pointBodyAt(entity, x, y)
	entity.x = x - entity.size_x * 0.5
	entity.y = y - entity.size_y * 0.5
end

local function entityPivotFractions(entity)
	return entity.rotation_pivot_x or 0.5, entity.rotation_pivot_y or 0.5
end

local function pointEntityPivotAt(entity, x, y)
	local px, py = entityPivotFractions(entity)
	entity.x = x - entity.size_x * px
	entity.y = y - entity.size_y * py
end

local function aimLine(entity, ax, ay, bx, by, thickness)
	local dx = bx - ax
	local dy = by - ay
	entity.x = ax
	entity.y = ay
	entity.size_x = math.sqrt(dx * dx + dy * dy)
	entity.size_y = thickness or 3
	entity.rotation = math.atan2(dy, dx)
end

local function applyTargetTransform()
	target.entity.x = targetX - target.entity.size_x * 0.5
	target.entity.y = targetY - target.entity.size_y * 0.5
	target.entity.rotation = targetRotation
	pointBodyAt(targetPivot, targetX, targetY)
end

local function resetDynamicBodies()
	for i = 1, #bolts do
		local item = bolts[i]
		local pinX, pinY = targetOffsetPoint(item.offsetX, item.offsetY)
		pointEntityPivotAt(item.body.entity, pinX + item.startX, pinY + item.startY)
		item.body.entity.rotation = item.startRotation
		item.body.rb:setVelocity(item.startVx, item.startVy)
		item.body.rb:setAngularVelocity(item.startOmega)
	end
end

local function createBoltedBody(label, offsetX, offsetY, strength, color, startX, startY)
	local pinX, pinY = targetOffsetPoint(offsetX, offsetY)
	local body = makePhysicsBody(
		"bolted_" .. label,
		pinX + startX,
		pinY + startY,
		108,
		26,
		"box",
		color,
		false
	)
	body.entity.rotation_pivot_x = 0
	body.entity.rotation_pivot_y = 0.5
	pointEntityPivotAt(body.entity, pinX + startX, pinY + startY)
	body.rb:setVelocity(startX * 0.35, -110)
	body.rb:addAngularImpulse((1 - strength) * 5 - 1.5)

	local bolt = ecs.addComponent(body.entity, core.Bolt2D)
	bolt:attach(target.entity)
	bolt.x = offsetX
	bolt.y = offsetY
	bolt.strength = strength
	bolt.contacts_enabled = false

	local guideLine = makeLine("guide_" .. label, Color4(88, 105, 135, 190), -3)
	local boltLine = makeLine("bolt_line_" .. label, Color4(color.r, color.g, color.b, 230), -2)
	local marker, markerDraw = makeShape(
		"pin_" .. label,
		pinX - 6,
		pinY - 6,
		12,
		12,
		"circle",
		Color4(color.r, color.g, color.b, 255),
		1
	)
	local strengthLabel = makeLabel(
		"strength_" .. label,
		pinX + startX + 58,
		pinY + startY - 14,
		15,
		Color4(color.r, color.g, color.b, 255),
		string.format("%.2f", strength)
	)

	local item = {
		label = label,
		offsetX = offsetX,
		offsetY = offsetY,
		strength = strength,
		body = body,
		bolt = bolt,
		guideLine = guideLine,
		boltLine = boltLine,
		marker = marker,
		markerDraw = markerDraw,
		strengthLabel = strengthLabel,
		startX = startX,
		startY = startY,
		startVx = startX * 0.35,
		startVy = -110,
		startOmega = (1 - strength) * 5 - 1.5,
		startRotation = 0,
	}
	bolts[#bolts + 1] = item
	return item
end

target = makePhysicsBody(
	"bolt_target",
	targetX - 70,
	targetY - 24,
	140,
	48,
	"box",
	Color4(106, 145, 245, 255),
	true
)
target.rb.gravity_y = 0
target.collider.is_trigger = true

targetPivot = makeShape("target_pivot", targetX - 7, targetY - 7, 14, 14, "circle", Color4(236, 242, 255, 255), 2)
applyTargetTransform()

createBoltedBody("hard", -180, -90, 1.0, Color4(95, 218, 185, 255), -80, 155)
createBoltedBody("medium", 0, 120, 0.55, Color4(255, 202, 92, 255), 120, 105)
createBoltedBody("strength_065", -90, 155, 0.65, Color4(176, 230, 112, 255), -130, 95)
createBoltedBody("strength_075", 90, 155, 0.75, Color4(105, 210, 245, 255), 115, 125)
createBoltedBody("strength_090", 0, -155, 0.9, Color4(184, 142, 255, 255), 35, 175)
createBoltedBody("soft", 180, -90, 0.22, Color4(240, 116, 126, 255), 75, 185)

makeLabel("title", 18, 24, 30, Color4(232, 238, 248, 255), "Bolt2D")
makeLabel(
	"hint",
	18,
	58,
	18,
	Color4(174, 190, 215, 255),
	"Blue body is the target. Small dots are Bolt2D x/y offsets from its pivot; bars are pinned at their left end."
)
makeLabel(
	"hint2",
	18,
	80,
	18,
	Color4(148, 168, 202, 255),
	"Move target: WASD/arrows. Rotate: Q/E. R resets bodies. Labels show each bolt strength."
)
local stats = makeLabel("stats", 18, 108, 17, Color4(126, 148, 184, 255), "")

ecs.addSystem({
	update = function(_self, dt)
		local moveX = 0
		local moveY = 0
		if input.isKeyDown("a") or input.isKeyDown("left") then
			moveX -= 1
		end
		if input.isKeyDown("d") or input.isKeyDown("right") then
			moveX += 1
		end
		if input.isKeyDown("w") or input.isKeyDown("up") then
			moveY -= 1
		end
		if input.isKeyDown("s") or input.isKeyDown("down") then
			moveY += 1
		end

		local moveSpeed = 280
		targetX += moveX * moveSpeed * dt
		targetY += moveY * moveSpeed * dt

		if input.isKeyDown("q") then
			targetRotation -= 1.9 * dt
		end
		if input.isKeyDown("e") then
			targetRotation += 1.9 * dt
		end

		if input.isMouseDown("left") then
			targetX += (mouse.x - targetX) * math.min(1, dt * 5)
			targetY += (mouse.y - targetY) * math.min(1, dt * 5)
		end

		targetX = math.clamp(targetX, 220, window.x - 220)
		targetY = math.clamp(targetY, 170, window.y - 250)
		applyTargetTransform()

		if input.isKeyPressed("r") then
			resetDynamicBodies()
		end

		local pivotX, pivotY = bodyPivot(target.entity)
		local totalForce = 0
		for i = 1, #bolts do
			local item = bolts[i]
			local pinX, pinY = targetOffsetPoint(item.offsetX, item.offsetY)
			local bodyX, bodyY = bodyPivot(item.body.entity)
			pointBodyAt(item.marker, pinX, pinY)
			item.strengthLabel.entity.x = bodyX + 62
			item.strengthLabel.entity.y = bodyY - 12
			aimLine(item.guideLine, pivotX, pivotY, pinX, pinY, 2)
			aimLine(item.boltLine, pinX, pinY, bodyX, bodyY, 5)
			totalForce += item.bolt.current_force

			local pulse = math.min(1, item.bolt.current_force / 5200)
			item.body.draw.color = Color4(
				item.body.baseColor.r + (255 - item.body.baseColor.r) * pulse,
				item.body.baseColor.g + (255 - item.body.baseColor.g) * pulse,
				item.body.baseColor.b + (255 - item.body.baseColor.b) * pulse,
				255
			)
		end

		local fps = dt > 0 and math.floor(1 / dt + 0.5) or 0
		stats.text = string.format(
			"Bolts: %d  Total force: %.0f  Target: %.0f, %.0f  FPS: %d",
			#bolts,
			totalForce,
			targetX,
			targetY,
			fps
		)
	end,
})
