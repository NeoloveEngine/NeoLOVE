app.bg = Color4(9, 11, 18, 255)
app.setMaxFps(120)

local WHITE = Color4(255, 255, 255, 255)
local MUTED = Color4(160, 172, 188, 255)
local BLUE = Color4(78, 160, 255, 255)
local GREEN = Color4(84, 220, 150, 255)
local AMBER = Color4(255, 190, 76, 255)
local RED = Color4(255, 94, 110, 255)
local TRANSPARENT = Color4(0, 0, 0, 0)
local TINT_IDLE = Color4(210, 220, 232, 165)
local TINT_HOVER = Color4(255, 255, 255, 230)
local TINT_SELECTED = Color4(255, 255, 255, 255)
local TINT_PROBE = Color4(255, 255, 255, 190)

local function setPixel(image, x, y, color)
	image:setPixel(x, y, color)
end

local function makeNineSliceImage()
	local image = assets.newImage(18, 18, TRANSPARENT)
	for y = 0, 17 do
		for x = 0, 17 do
			local inOuter = x == 0 or x == 17 or y == 0 or y == 17
			local inInner = x == 3 or x == 14 or y == 3 or y == 14
			local corner = (x < 4 or x > 13) and (y < 4 or y > 13)
			if inOuter then
				setPixel(image, x, y, Color4(92, 111, 145, 255))
			elseif inInner and not corner then
				setPixel(image, x, y, Color4(30, 39, 58, 255))
			else
				setPixel(image, x, y, Color4(19, 25, 39, 232))
			end
		end
	end
	image:upload()
	return image
end

local function makeCircleImage(color)
	local image = assets.newImage(64, 64, TRANSPARENT)
	for y = 0, 63 do
		for x = 0, 63 do
			local dx = x - 31.5
			local dy = y - 31.5
			local dist = math.sqrt(dx * dx + dy * dy)
			if dist <= 24 then
				setPixel(image, x, y, color)
			elseif dist <= 26 then
				setPixel(image, x, y, Color4(255, 255, 255, 95))
			end
		end
	end
	image:upload()
	return image
end

local function makeDiamondImage(color)
	local image = assets.newImage(64, 64, TRANSPARENT)
	for y = 0, 63 do
		for x = 0, 63 do
			local dx = math.abs(x - 31.5)
			local dy = math.abs(y - 31.5)
			local d = dx + dy
			if d <= 28 then
				setPixel(image, x, y, color)
			elseif d <= 30 then
				setPixel(image, x, y, Color4(255, 255, 255, 90))
			end
		end
	end
	image:upload()
	return image
end

local function makeRingImage(color)
	local image = assets.newImage(64, 64, TRANSPARENT)
	for y = 0, 63 do
		for x = 0, 63 do
			local dx = x - 31.5
			local dy = y - 31.5
			local dist = math.sqrt(dx * dx + dy * dy)
			if dist >= 14 and dist <= 26 then
				setPixel(image, x, y, color)
			elseif dist > 26 and dist <= 28 then
				setPixel(image, x, y, Color4(255, 255, 255, 70))
			end
		end
	end
	image:upload()
	return image
end

local function makeCrescentImage(color)
	local image = assets.newImage(64, 64, TRANSPARENT)
	for y = 0, 63 do
		for x = 0, 63 do
			local dx = x - 30
			local dy = y - 31.5
			local outer = math.sqrt(dx * dx + dy * dy)
			local cutDx = x - 42
			local cut = math.sqrt(cutDx * cutDx + dy * dy)
			if outer <= 27 and cut > 21 then
				setPixel(image, x, y, color)
			elseif outer <= 29 and cut > 20 then
				setPixel(image, x, y, Color4(255, 255, 255, 80))
			end
		end
	end
	image:upload()
	return image
end

local function makeProbeImage()
	local image = assets.newImage(32, 32, TRANSPARENT)
	for y = 0, 31 do
		for x = 0, 31 do
			local dx = x - 15.5
			local dy = y - 15.5
			local dist = math.sqrt(dx * dx + dy * dy)
			if dist <= 10 then
				setPixel(image, x, y, Color4(255, 255, 255, 225))
			elseif dist <= 12 then
				setPixel(image, x, y, Color4(78, 160, 255, 145))
			end
		end
	end
	image:upload()
	return image
end

local panelImage = makeNineSliceImage()

local function makeText(name, x, y, w, h, text, scale, color)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	entity.size_x = w
	entity.size_y = h
	entity.z = 20
	entity.raycastable = false
	local label = ecs.addComponent(entity, core.TextBox)
	label.text = text
	label.scale = scale
	label.min_scale = 10
	label.size_mode = "entity"
	label.text_scale = "fit"
	label.wrap = "word"
	label.align_y = "center"
	label.color = color or WHITE
	return entity, label
end

local function makePanel(name, x, y, w, h, z)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	entity.size_x = w
	entity.size_y = h
	entity.z = z or -10
	entity.raycastable = false
	local renderer = ecs.addComponent(entity, core.NineSliceSprite2D)
	renderer.image = panelImage
	renderer.slice_left = 4
	renderer.slice_right = 4
	renderer.slice_top = 4
	renderer.slice_bottom = 4
	return entity, renderer
end

local rootPanel = makePanel("demo_panel", 28, 24, 840, 520, -20)
local smallPanel = ecs.newEntity("alias_panel", ecs.root, 588, 368)
smallPanel.size_x = 238
smallPanel.size_y = 132
smallPanel.z = -12
smallPanel.raycastable = false
local aliasNineSlice = ecs.addComponent(smallPanel, core["9SliceSprite2D"])
aliasNineSlice.image = panelImage
aliasNineSlice.sliceLeft = 4
aliasNineSlice.sliceRight = 4
aliasNineSlice.sliceTop = 4
aliasNineSlice.sliceBottom = 4

local titleEntity, title = makeText("title", 58, 42, 780, 54, "Spritebox2D + 9SliceSprite2D demo", 36, WHITE)
title.align_x = "center"
local helpEntity, help = makeText(
	"help",
	66,
	92,
	764,
	56,
	"Move the mouse. The blue probe intersects only opaque sprite pixels. Click a colored shape: transparent padding is ignored.",
	20,
	MUTED
)
help.align_x = "center"

local statusEntity, status = makeText("status", 66, 492, 764, 34, "", 20, AMBER)
status.align_x = "center"

local targets = {}
local selectedName = "none"

local function makeTarget(name, image, x, y, color)
	local holder = ecs.newEntity(name, ecs.root, x, y)
	holder.size_x = 96
	holder.size_y = 96
	holder.z = 4
	holder.raycastable = false

	local sprite = ecs.addComponent(holder, core.Sprite2D)
	sprite.image = image
	sprite.color = TINT_IDLE

	local spritebox = ecs.addComponent(holder, core.Spritebox2D)
	spritebox.alpha_threshold = 96
	spritebox:ComputeSpritebox()

	local captionEntity, caption = makeText(name .. "_caption", x - 10, y + 104, 116, 24, name, 17, MUTED)
	caption.align_x = "center"

	local item = {
		name = name,
		entity = holder,
		sprite = sprite,
		spritebox = spritebox,
		caption = caption,
		color = color,
		hover = false,
		intersecting = false,
		visual = "",
	}
	targets[#targets + 1] = item
	return item
end

makeTarget("circle", makeCircleImage(BLUE), 98, 190, BLUE)
makeTarget("diamond", makeDiamondImage(GREEN), 264, 190, GREEN)
makeTarget("ring", makeRingImage(AMBER), 430, 190, AMBER)
makeTarget("crescent", makeCrescentImage(RED), 596, 190, RED)

local probeEntity = ecs.newEntity("probe", ecs.root, 0, 0)
probeEntity.size_x = 42
probeEntity.size_y = 42
probeEntity.z = 15
probeEntity.raycastable = false
local probeSprite = ecs.addComponent(probeEntity, core.Sprite2D)
probeSprite.image = makeProbeImage()
probeSprite.color = TINT_PROBE
local probeBox = ecs.addComponent(probeEntity, core.Spritebox2D)
probeBox.alpha_threshold = 48
probeBox:ComputeSpritebox()

local aliasTitleEntity, aliasTitle = makeText("alias_title", 610, 384, 194, 26, "core[\"9SliceSprite2D\"]", 18, WHITE)
aliasTitle.align_x = "center"
local aliasBodyEntity, aliasBody = makeText(
	"alias_body",
	612,
	416,
	190,
	58,
	"This smaller panel is drawn through the numeric nine-slice alias.",
	15,
	MUTED
)
aliasBody.align_x = "center"

local function setTint(item)
	local nextVisual
	if selectedName == item.name then
		nextVisual = "selected"
	elseif item.intersecting then
		nextVisual = "intersecting"
	elseif item.hover then
		nextVisual = "hover"
	else
		nextVisual = "idle"
	end

	if item.visual == nextVisual then
		return
	end
	item.visual = nextVisual

	if nextVisual == "selected" then
		item.sprite.color = TINT_SELECTED
		item.caption.color = WHITE
	elseif nextVisual == "intersecting" then
		item.sprite.color = Color4(item.color.r, item.color.g, item.color.b, 255)
		item.caption.color = item.color
	elseif nextVisual == "hover" then
		item.sprite.color = TINT_HOVER
		item.caption.color = WHITE
	else
		item.sprite.color = TINT_IDLE
		item.caption.color = MUTED
	end
end

local targetOffsets = {
	{ x = 70, y = 166 },
	{ x = 236, y = 166 },
	{ x = 402, y = 166 },
	{ x = 568, y = 166 },
}
local lastLayoutX = nil
local lastLayoutY = nil
local lastStatus = ""

ecs.addSystem({
	update = function(self, dt)
		rootPanel.x = math.max(18, (window.x - rootPanel.size_x) * 0.5)
		rootPanel.y = math.max(16, (window.y - rootPanel.size_y) * 0.5)

		local baseX = rootPanel.x
		local baseY = rootPanel.y
		if baseX ~= lastLayoutX or baseY ~= lastLayoutY then
			lastLayoutX = baseX
			lastLayoutY = baseY
			titleEntity.x = baseX + 30
			titleEntity.y = baseY + 18
			helpEntity.x = baseX + 38
			helpEntity.y = baseY + 68
			statusEntity.x = baseX + 38
			statusEntity.y = baseY + 468

			for index, item in ipairs(targets) do
				local offset = targetOffsets[index]
				local x = baseX + offset.x
				local y = baseY + offset.y
				item.entity.x = x
				item.entity.y = y
				item.caption.entity.x = x - 10
				item.caption.entity.y = y + 104
			end

			smallPanel.x = baseX + 560
			smallPanel.y = baseY + 344
			aliasTitleEntity.x = smallPanel.x + 22
			aliasTitleEntity.y = smallPanel.y + 16
			aliasBodyEntity.x = smallPanel.x + 24
			aliasBodyEntity.y = smallPanel.y + 48
		end

		probeEntity.x = mouse.x - probeEntity.size_x * 0.5
		probeEntity.y = mouse.y - probeEntity.size_y * 0.5

		local hoverName = "none"
		local intersectText = ""
		for _, item in ipairs(targets) do
			item.hover = item.spritebox:IsInside(mouse.x, mouse.y)
			item.intersecting = probeBox:IsIntersecting(item.spritebox)
			if item.hover then
				hoverName = item.name
			end
			if item.intersecting then
				if intersectText == "" then
					intersectText = item.name
				else
					intersectText = intersectText .. ", " .. item.name
				end
			end

			if input.isMousePressed("left") and item.hover then
				selectedName = item.name
			end
			setTint(item)
		end

		if intersectText == "" then
			intersectText = "none"
		end
		local nextStatus = string.format(
			"hover=%s | selected=%s | probe intersections=%s",
			hoverName,
			selectedName,
			intersectText
		)
		if nextStatus ~= lastStatus then
			lastStatus = nextStatus
			status.text = nextStatus
		end
	end,
})
