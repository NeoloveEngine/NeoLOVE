app.bg = Color4(15, 19, 28, 255)
app.setShowFps(true)

local logLines = {}

local function pushLog(message)
	table.insert(logLines, 1, message)
	while #logLines > 9 do
		table.remove(logLines)
	end
end

local function makePanel(name, x, y, w, h, color)
	local entity = ecs.newEntity(name, ecs.root, x, y)
	entity.size_x = w
	entity.size_y = h

	local shape = entity:AddComponent(core.Shape2D)
	shape.shape = "box"
	shape.color = color

	return entity, shape
end

local function makeText(name, parent, x, y, w, h, text, scale, color)
	local entity = ecs.newEntity(name, parent, x, y)
	entity.size_x = w
	entity.size_y = h

	local label = entity:AddComponent(core.TextBox)
	label.text = text
	label.scale = scale
	label.color = color
	label.size_mode = "entity"
	label.wrap = "word"
	label.text_scale = "fit"
	label.align_x = "left"
	label.align_y = "top"
	label.padding_x = 10
	label.padding_y = 8

	return entity, label
end

local leftPanel = makePanel("leftPanel", 32, 36, 328, 340, Color4(30, 38, 56, 244))
local rightPanel = makePanel("rightPanel", 392, 36, 360, 340, Color4(24, 29, 40, 244))

local titleEntity, titleText = makeText(
	"title",
	leftPanel,
	18,
	14,
	292,
	42,
	"Entity :Listen() + Instance Methods",
	30,
	Color4(244, 247, 255, 255)
)
titleText.align_x = "center"
titleText.align_y = "center"

local subtitleEntity, subtitleText = makeText(
	"subtitle",
	leftPanel,
	18,
	54,
	292,
	48,
	"Hover the card below. Left, right, middle, scroll up, and scroll down all do something different.",
	18,
	Color4(186, 202, 231, 255)
)

local card = ecs.newEntity("listenerCard", leftPanel, 18, 112)
card.size_x = 292
card.size_y = 160

local cardBackground = card:AddComponent(core.Shape2D)
cardBackground.shape = "box"
cardBackground.color = Color4(56, 88, 142, 255)

local cardTitle = card:AddComponent(core.TextBox)
cardTitle.text = "Listener Card"
cardTitle.scale = 28
cardTitle.color = Color4(255, 255, 255, 255)
cardTitle.size_mode = "entity"
cardTitle.align_x = "center"
cardTitle.align_y = "top"
cardTitle.text_scale = "fit_width"
cardTitle.padding_y = 12
cardTitle.padding_x = 14

local cardBody = card:AddComponent(core.TextBox)
cardBody.text = "Left click nudges the card.\nRight click duplicates the badge.\nMiddle click removes/restores a component.\nMouse wheel spins the orange dial."
cardBody.scale = 16
cardBody.color = Color4(224, 232, 246, 255)
cardBody.size_mode = "entity"
cardBody.align_x = "left"
cardBody.align_y = "center"
cardBody.padding_x = 18
cardBody.padding_y = 12
cardBody.wrap = "word"
cardBody.line_spacing = 1.1

local hintComponent
local function addHintComponent()
	hintComponent = card:AddComponent(core.TextBox)
	hintComponent.text = "Middle click toggles this component via :Remove()"
	hintComponent.scale = 14
	hintComponent.color = Color4(255, 236, 169, 255)
	hintComponent.size_mode = "entity"
	hintComponent.align_x = "center"
	hintComponent.align_y = "bottom"
	hintComponent.text_scale = "fit_width"
	hintComponent.padding_x = 14
	hintComponent.padding_y = 10
end

addHintComponent()

local scrollDial = ecs.newEntity("scrollDial", card, 228, 64)
scrollDial.size_x = 42
scrollDial.size_y = 42
scrollDial.rotation_pivot = "middle"
scrollDial.rotation = 0.35

local scrollDialShape = scrollDial:AddComponent(core.Shape2D)
scrollDialShape.shape = "box"
scrollDialShape.color = Color4(255, 181, 54, 255)

local scrollDialCore = ecs.newEntity("scrollDialCore", scrollDial, 10, 10)
scrollDialCore.size_x = 22
scrollDialCore.size_y = 22

local scrollDialCoreShape = scrollDialCore:AddComponent(core.Shape2D)
scrollDialCoreShape.shape = "box"
scrollDialCoreShape.color = Color4(32, 37, 49, 255)

local badge = ecs.newEntity("badge", card, 206, 18)
badge.size_x = 68
badge.size_y = 30

local badgeShape = badge:AddComponent(core.Shape2D)
badgeShape.shape = "box"
badgeShape.color = Color4(255, 181, 54, 255)

local badgeText = badge:AddComponent(core.TextBox)
badgeText.text = "ORIG"
badgeText.scale = 18
badgeText.color = Color4(27, 22, 13, 255)
badgeText.size_mode = "entity"
badgeText.align_x = "center"
badgeText.align_y = "center"
badgeText.text_scale = "fit_width"
badgeText.padding_x = 6
badgeText.padding_y = 2

local deleteChip = ecs.newEntity("deleteChip", card, 16, 118)
deleteChip.size_x = 116
deleteChip.size_y = 28

local deleteChipShape = deleteChip:AddComponent(core.Shape2D)
deleteChipShape.shape = "box"
deleteChipShape.color = Color4(173, 56, 84, 255)

local deleteChipText = deleteChip:AddComponent(core.TextBox)
deleteChipText.text = "LEFT: DELETE ME"
deleteChipText.scale = 14
deleteChipText.color = Color4(255, 244, 246, 255)
deleteChipText.size_mode = "entity"
deleteChipText.align_x = "center"
deleteChipText.align_y = "center"
deleteChipText.text_scale = "fit_width"
deleteChipText.padding_x = 8
deleteChipText.padding_y = 2

local disconnectButton = ecs.newEntity("disconnectButton", rightPanel, 18, 18)
disconnectButton.size_x = 324
disconnectButton.size_y = 68

local disconnectShape = disconnectButton:AddComponent(core.Shape2D)
disconnectShape.shape = "box"
disconnectShape.color = Color4(64, 120, 85, 255)

local disconnectText = disconnectButton:AddComponent(core.TextBox)
disconnectText.text = "Click here to disconnect the secondary left-click listener"
disconnectText.scale = 20
disconnectText.color = Color4(238, 251, 241, 255)
disconnectText.size_mode = "entity"
disconnectText.align_x = "center"
disconnectText.align_y = "center"
disconnectText.wrap = "word"
disconnectText.text_scale = "fit"
disconnectText.padding_x = 16
disconnectText.padding_y = 8

local stateEntity, stateText = makeText(
	"stateText",
	rightPanel,
	18,
	100,
	324,
	96,
	"",
	16,
	Color4(230, 239, 255, 255)
)
stateText.line_spacing = 1.1

local logTitleEntity, logTitleText = makeText(
	"logTitle",
	rightPanel,
	18,
	202,
	324,
	28,
	"Event Log",
	24,
	Color4(249, 250, 252, 255)
)
logTitleText.align_y = "center"

local logEntity, logText = makeText(
	"logText",
	rightPanel,
	18,
	234,
	324,
	88,
	"",
	15,
	Color4(198, 209, 228, 255)
)
logText.line_spacing = 1.15

local leftClicks = 0
local echoClicks = 0
local badgeCopies = 0
local cardHue = false
local disconnected = false

local secondaryConnection

card:Listen("leftClick", function(entity, event)
	leftClicks += 1
	cardHue = not cardHue
	entity.x = entity.x + 4
	entity.y = entity.y + 2
	cardBackground.color = cardHue and Color4(84, 128, 200, 255) or Color4(56, 88, 142, 255)
	pushLog(string.format("leftClick #%d at %.0f, %.0f", leftClicks, event.mouseX, event.mouseY))
end)

secondaryConnection = card:Listen("leftClick", function(entity, event)
	echoClicks += 1
	local x, y = entity:GetWorldPosition()
	pushLog(string.format("secondary listener #%d at world %.1f, %.1f", echoClicks, x, y))
end)

card:Listen("rightClick", function(entity, event)
	local originalBadge = entity:FindFirstChild("badge")
	if originalBadge == nil then
		pushLog("rightClick: original badge no longer exists")
		return
	end

	badgeCopies += 1
	local copy = originalBadge:Duplicate(entity)
	copy.name = "badge_copy_" .. tostring(badgeCopies)
	copy.x = 206
	copy.y = 18 + (badgeCopies * 34)
	if copy.y > 120 then
		copy.y = 52 + ((badgeCopies - 1) % 3) * 34
	end

	local copyText = copy.components[2]
	if copyText ~= nil then
		copyText.text = "COPY " .. tostring(badgeCopies)
	end

	pushLog(string.format("rightClick: duplicated badge via :Duplicate() (%d copies)", badgeCopies))
end)

card:Listen("middleClick", function(entity, event)
	if hintComponent ~= nil and hintComponent:GetEntity() ~= nil then
		local owner = hintComponent:GetEntity()
		hintComponent:Remove()
		hintComponent = nil
		pushLog("middleClick: removed hint component from " .. owner.name)
	else
		addHintComponent()
		pushLog("middleClick: restored hint component with :AddComponent()")
	end
end)

card:Listen("scrollUp", function(entity, event)
	scrollDial.rotation = scrollDial.rotation - 0.12
	pushLog(string.format("scrollUp: dial rotation -> %.2f", scrollDial:GetWorldRotation()))
end)

card:Listen("scrollDown", function(entity, event)
	scrollDial.rotation = scrollDial.rotation + 0.12
	pushLog(string.format("scrollDown: dial rotation -> %.2f", scrollDial:GetWorldRotation()))
end)

deleteChip:listen("leftClick", function(entity, event)
	pushLog(entity.name .. ":Delete()")
	entity:Delete()
end)

disconnectButton:Listen("leftClick", function(entity, event)
	if secondaryConnection ~= nil and secondaryConnection:IsConnected() then
		secondaryConnection:Disconnect()
		disconnected = true
		disconnectShape.color = Color4(84, 86, 92, 255)
		disconnectText.text = "Secondary listener disconnected"
		pushLog("connection:Disconnect() disabled the secondary left-click listener")
	else
		pushLog("secondary listener is already disconnected")
	end
end)

pushLog("Hover the blue card and try every mouse action.")
pushLog("Click the green button to disconnect the extra left-click listener.")
pushLog("The red chip inside the card deletes itself.")

ecs.addSystem({
	update = function(self, dt)
		local wx, wy = card:GetWorldPosition()
		local dialRotation = scrollDial:GetWorldRotation()
		local hintActive = hintComponent ~= nil and hintComponent:GetEntity() ~= nil
		local chipAlive = card:FindFirstChild("deleteChip") ~= nil
		local secondaryConnected = secondaryConnection ~= nil and secondaryConnection:IsConnected()

		stateText.text = string.format(
			"world position: %.1f, %.1f\ndial rotation: %.2f\nsecondary connected: %s\nhint component active: %s\ndelete chip alive: %s\nbadge copies: %d",
			wx,
			wy,
			dialRotation,
			tostring(secondaryConnected),
			tostring(hintActive),
			tostring(chipAlive),
			badgeCopies
		)

		logText.text = table.concat(logLines, "\n")

		leftPanel.x = math.max(20, (window.x * 0.5) - 360)
		rightPanel.x = leftPanel.x + 360
		leftPanel.y = math.max(20, (window.y - leftPanel.size_y) * 0.5)
		rightPanel.y = leftPanel.y

		if disconnected then
			disconnectButton.rotation = math.min(disconnectButton.rotation + dt, 0.08)
		else
			disconnectButton.rotation = math.max(disconnectButton.rotation - dt, 0.0)
		end
	end
})
