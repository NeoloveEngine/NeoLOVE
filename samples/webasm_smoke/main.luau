app.bg = Color4(14, 18, 26, 255)
app.setMaxFps(60)
app.setShowFps(false)

local HTTP_TEST_URL = "https://example.com/"
local IMAGE_SIZE = 72
local SOUND_RATE = 24000
local SOUND_DURATION = 0.38
local MAX_LOG_LINES = 10
local CARD_GAP = 16
local CARD_MAX_WIDTH = 180

local state = {
	assetText = "pending",
	fs = "idle - press F",
	http = "idle - press H",
	loadedImage = "pending",
	generatedImage = "pending",
	reloadedImage = "idle - press X",
	loadedSound = "pending",
	generatedSound = "pending",
	reloadedSound = "idle - press X",
	exports = "idle - press X",
}

local logLines = {}
local manifestPreview = "pending"
local loadedImage = nil
local generatedImage = nil
local reloadedImage = nil
local loadedSound = nil
local generatedSound = nil
local reloadedSound = nil
local uiDirty = true
local lastWindowWidth = -1
local lastWindowHeight = -1
local imageTime = 0.0

local function short(text, limit)
	local normalized = tostring(text or ""):gsub("[%c]+", " ")
	if #normalized <= limit then
		return normalized
	end
	return normalized:sub(1, math.max(0, limit - 3)) .. "..."
end

local function markUiDirty()
	uiDirty = true
end

local function addLog(message)
	local rendered = short(message, 120)
	print(rendered)
	table.insert(logLines, rendered)
	while #logLines > MAX_LOG_LINES do
		table.remove(logLines, 1)
	end
	markUiDirty()
end

local function safeRun(label, action)
	local ok, result = pcall(action)
	if not ok then
		addLog(label .. " failed: " .. short(result, 96))
		return false, result
	end
	return true, result
end

local function rgbaString(color)
	if not color then
		return "?, ?, ?, ?"
	end
	return string.format("%d, %d, %d, %d", color.r or 0, color.g or 0, color.b or 0, color.a or 0)
end

local function setState(key, value)
	if state[key] ~= value then
		state[key] = value
		markUiDirty()
	end
end

local function buildGeneratedImage()
	local image = assets.newImage(IMAGE_SIZE, IMAGE_SIZE, Color4(10, 12, 18, 255))
	local center = (IMAGE_SIZE - 1) * 0.5
	for y = 0, IMAGE_SIZE - 1 do
		for x = 0, IMAGE_SIZE - 1 do
			local checker = ((math.floor(x / 8) + math.floor(y / 8)) % 2) == 0
			local dx = x - center
			local dy = y - center
			local distance = math.sqrt(dx * dx + dy * dy) / (IMAGE_SIZE * 0.72)
			local glow = math.clamp(1.0 - distance, 0.0, 1.0)
			local red = checker and (28 + glow * 176) or (14 + glow * 82)
			local green = checker and (104 + glow * 112) or (40 + glow * 56)
			local blue = checker and (150 + glow * 90) or (72 + glow * 92)
			if math.abs(dx) < 3 or math.abs(dy) < 3 then
				red = 236
				green = 246
				blue = 214
			end
			image:setPixel(x, y, Color4(red, green, blue, 255))
		end
	end
	image:upload()
	return image
end

local function buildGeneratedSound()
	local sampleCount = math.floor(SOUND_RATE * SOUND_DURATION)
	local sound = assets.newSound(SOUND_RATE, 1, sampleCount, 0)
	for i = 0, sound:len() - 1 do
		local t = i / SOUND_RATE
		local fade = math.max(0, 1 - t / SOUND_DURATION)
		local wobble = math.sin(t * math.pi * 2 * 2.5) * 18
		local toneA = math.sin(t * math.pi * 2 * (440 + wobble))
		local toneB = math.sin(t * math.pi * 2 * 660) * 0.35
		local sample = (toneA + toneB) * fade * 0.42
		sound:setSample(i, sample)
	end
	sound:upload()
	return sound
end

local function playSound(label, sound, volume)
	if not sound then
		addLog(label .. " sound handle is missing")
		return
	end

	local ok = safeRun("audio " .. label, function()
		audio.playOnce(sound, volume or 0.75)
	end)
	if ok then
		addLog("played " .. label)
	end
end

local function runFsProbe()
	local ok = safeRun("fs probe", function()
		fs.createDir("runtime")
		fs.createDir("runtime/nested")
		fs.writeFile("runtime/fs_probe.txt", "alpha")
		fs.appendFile("runtime/fs_probe.txt", "\nbeta")
		local content = fs.readFile("runtime/fs_probe.txt")
		fs.copy("runtime/fs_probe.txt", "runtime/nested/fs_probe_copy.txt")
		fs.rename("runtime/nested/fs_probe_copy.txt", "runtime/fs_probe_renamed.txt")

		local exists = fs.exists("runtime/fs_probe_renamed.txt")
		local isFile = fs.isFile("runtime/fs_probe_renamed.txt")
		local isDir = fs.isDir("runtime/nested")
		local entries = fs.walk("runtime", true)
		local preview = {}
		for index, entry in ipairs(entries) do
			if index > 6 then
				break
			end
			table.insert(preview, entry.path)
		end

		local removed = fs.removeFile("runtime/fs_probe_renamed.txt")
		local stillExists = fs.exists("runtime/fs_probe_renamed.txt")
		setState(
			"fs",
			string.format(
				"ok read=%s exists=%s file=%s dir=%s removed=%s gone=%s walk=%s",
				short(content, 18),
				tostring(exists),
				tostring(isFile),
				tostring(isDir),
				tostring(removed),
				tostring(not stillExists),
				short(table.concat(preview, ", "), 72)
			)
		)
		addLog("fs probe ok")
	end)

	if not ok then
		setState("fs", "error")
	end
end

local function runHttpProbe()
	local ok, requestIdOrError = safeRun("http probe", function()
		return http.get(HTTP_TEST_URL, function(response)
			if response.ok then
				local body = response.body or ""
				setState(
					"http",
					string.format(
						"callback ok status=%s chars=%d body=%s",
						tostring(response.status),
						#body,
						short(body, 52)
					)
				)
				addLog("http callback ok " .. tostring(response.status))
			else
				setState("http", "callback error: " .. short(response.error or response.status, 72))
				addLog("http callback error")
			end
		end)
	end)

	if ok then
		setState("http", "queued request #" .. tostring(requestIdOrError))
		addLog("http queued " .. tostring(requestIdOrError))
	else
		setState("http", short(requestIdOrError, 84))
	end
end

local function runExportProbe()
	if not generatedImage or not generatedSound then
		setState("exports", "blocked - generated assets missing")
		setState("reloadedImage", "blocked")
		setState("reloadedSound", "blocked")
		return
	end

	local ok = safeRun("asset export probe", function()
		fs.createDir("runtime")
		generatedImage:save("runtime/generated_preview")
		generatedSound:save("runtime/generated_beep")

		local exportedImageExists = fs.exists("runtime/generated_preview.png")
		local exportedSoundExists = fs.exists("runtime/generated_beep.wav")
		reloadedImage = assets.loadImage("./runtime/generated_preview.png")
		reloadedSound = assets.loadSound("./runtime/generated_beep.wav")

		local reloadedPixel = reloadedImage:getPixel(0, 0)
		setState(
			"reloadedImage",
			string.format(
				"%dx%d top-left rgba(%s)",
				reloadedImage:width(),
				reloadedImage:height(),
				rgbaString(reloadedPixel)
			)
		)
		setState(
			"reloadedSound",
			string.format(
				"%dHz %dch %d samples",
				reloadedSound:sampleRate(),
				reloadedSound:channels(),
				reloadedSound:len()
			)
		)
		setState(
			"exports",
			string.format(
				"png=%s wav=%s",
				tostring(exportedImageExists),
				tostring(exportedSoundExists)
			)
		)
		addLog("asset export + reload ok")
	end)

	if not ok then
		setState("exports", "error")
		setState("reloadedImage", "unavailable")
		setState("reloadedSound", "unavailable")
	end
end

local panel = ecs.newEntity("panel", ecs.root, 0, 0)
panel.z = -10
local panelShape = ecs.addComponent(panel, core.Shape2D)
panelShape.shape = "box"
panelShape.color = Color4(26, 31, 42, 240)

local titleEntity = ecs.newEntity("title", ecs.root, 0, 0)
titleEntity.z = 10
titleEntity.size_x = 720
titleEntity.size_y = 42
local titleText = ecs.addComponent(titleEntity, core.TextBox)
titleText.text = "NeoLOVE Webasm Smoke Sample"
titleText.scale = 34
titleText.size_mode = "entity"
titleText.text_scale = "fit_width"
titleText.align_x = "center"
titleText.align_y = "center"
titleText.color = Color4(240, 246, 255, 255)

local subtitleEntity = ecs.newEntity("subtitle", ecs.root, 0, 0)
subtitleEntity.z = 10
subtitleEntity.size_x = 720
subtitleEntity.size_y = 44
local subtitleText = ecs.addComponent(subtitleEntity, core.TextBox)
subtitleText.text = "Basic webasm smoke test for packaged assets, generated assets, fs writes, export/reload, audio playback, and current HTTP support."
subtitleText.scale = 18
subtitleText.color = Color4(173, 205, 222, 255)
subtitleText.size_mode = "entity"
subtitleText.wrap = "word"
subtitleText.text_scale = "fit"
subtitleText.align_x = "center"
subtitleText.align_y = "center"

local summaryEntity = ecs.newEntity("summary", ecs.root, 0, 0)
summaryEntity.z = 10
summaryEntity.size_x = 720
summaryEntity.size_y = 210
local summaryText = ecs.addComponent(summaryEntity, core.TextBox)
summaryText.scale = 14
summaryText.color = Color4(230, 236, 243, 255)
summaryText.size_mode = "entity"
summaryText.wrap = "word"

local logEntity = ecs.newEntity("log", ecs.root, 0, 0)
logEntity.z = 10
logEntity.size_x = 720
logEntity.size_y = 120
local logText = ecs.addComponent(logEntity, core.TextBox)
logText.scale = 13
logText.color = Color4(171, 220, 173, 255)
logText.size_mode = "entity"
logText.wrap = "word"

local controlsEntity = ecs.newEntity("controls", ecs.root, 0, 0)
controlsEntity.z = 10
controlsEntity.size_x = 720
controlsEntity.size_y = 64
local controlsText = ecs.addComponent(controlsEntity, core.TextBox)
controlsText.scale = 13
controlsText.color = Color4(255, 226, 168, 255)
controlsText.size_mode = "entity"
controlsText.wrap = "word"
controlsText.text_scale = "fit"

local assetPreviewEntity = ecs.newEntity("asset_preview", ecs.root, 0, 0)
assetPreviewEntity.z = 10
assetPreviewEntity.size_x = 720
assetPreviewEntity.size_y = 40
local assetPreviewText = ecs.addComponent(assetPreviewEntity, core.TextBox)
assetPreviewText.scale = 13
assetPreviewText.color = Color4(159, 182, 206, 255)
assetPreviewText.size_mode = "entity"
assetPreviewText.wrap = "word"

local cardFillColor = Color4(18, 23, 32, 255)

local function createImageCard(prefix, labelTextValue)
	local card = ecs.newEntity(prefix .. "_card", ecs.root, 0, 0)
	card.z = -1
	local cardShape = ecs.addComponent(card, core.Shape2D)
	cardShape.shape = "box"
	cardShape.color = cardFillColor

	local imageEntity = ecs.newEntity(prefix .. "_image", ecs.root, 0, 0)
	imageEntity.z = 0
	local imageRenderer = ecs.addComponent(imageEntity, core.Image2D)
	imageRenderer.color = Color4(255, 255, 255, 255)

	local labelEntity = ecs.newEntity(prefix .. "_label", ecs.root, 0, 0)
	labelEntity.z = 2
	labelEntity.size_x = 160
	labelEntity.size_y = 28
	local label = ecs.addComponent(labelEntity, core.TextBox)
	label.text = labelTextValue
	label.scale = 17
	label.color = Color4(208, 226, 240, 255)
	label.size_mode = "entity"
	label.text_scale = "fit_width"
	label.align_x = "center"
	label.align_y = "center"

	return {
		card = card,
		imageEntity = imageEntity,
		imageRenderer = imageRenderer,
		labelEntity = labelEntity,
		label = label,
	}
end

local assetCard = createImageCard("asset", "Loaded asset PNG")
local generatedCard = createImageCard("generated", "Procedural image")
local reloadedCard = createImageCard("reloaded", "Exported + reloaded PNG")

local function refreshUiText()
	summaryText.text = table.concat({
		string.format("Window: %dx%d | Assets: %s | Exports: %s", lastWindowWidth, lastWindowHeight, state.assetText, state.exports),
		"Images: loaded=" .. state.loadedImage,
		"Images: generated=" .. state.generatedImage,
		"Images: reloaded=" .. state.reloadedImage,
		"Sounds: loaded=" .. state.loadedSound,
		"Sounds: generated=" .. state.generatedSound,
		"Sounds: reloaded=" .. state.reloadedSound,
		"FS: " .. state.fs,
		"HTTP: " .. state.http,
	}, "\n")

	logText.text = "Recent log:\n" .. table.concat(logLines, "\n")
	assetPreviewText.text = "manifest.txt preview: " .. manifestPreview
	controlsText.text =
		"Keys: Space plays the bundled wav, B plays the generated beep, E plays the exported/reloaded wav, X runs export + reload, F runs fs checks, H runs the HTTP probe. Click once before expecting audio in the browser."

	uiDirty = false
end

local function bootstrap()
	addLog("booting sample")

	local ok, result = safeRun("load assets", function()
		manifestPreview = short(fs.readFile("assets/manifest.txt"), 100)
		setState("assetText", "ok")

		loadedImage = assets.loadImage("me.png")
		local firstPixel = loadedImage:getPixel(0, 0)
		setState(
			"loadedImage",
			string.format(
				"%dx%d top-left rgba(%s)",
				loadedImage:width(),
				loadedImage:height(),
				rgbaString(firstPixel)
			)
		)
		assetCard.imageRenderer.image = loadedImage

		loadedSound = assets.loadSound("sfx.wav")
		setState(
			"loadedSound",
			string.format(
				"%dHz %dch %d samples",
				loadedSound:sampleRate(),
				loadedSound:channels(),
				loadedSound:len()
			)
		)

		generatedImage = buildGeneratedImage()
		setState(
			"generatedImage",
			string.format(
				"%dx%d in-memory image",
				generatedImage:width(),
				generatedImage:height()
			)
		)
		generatedCard.imageRenderer.image = generatedImage

		generatedSound = buildGeneratedSound()
		setState(
			"generatedSound",
			string.format(
				"%dHz %dch %d samples",
				generatedSound:sampleRate(),
				generatedSound:channels(),
				generatedSound:len()
			)
		)

		addLog("packaged asset load ok")
		addLog("press X for export + reload")
		addLog("press F for fs and H for http")
	end)

	if not ok then
		setState("assetText", short(result, 84))
		setState("loadedImage", "error")
		setState("loadedSound", "error")
		setState("generatedImage", "error")
		setState("generatedSound", "error")
	end
end

bootstrap()

ecs.addSystem({
	update = function(_self, dt)
		imageTime += dt

		if input.isKeyPressed("space") then
			playSound("loaded asset", loadedSound, 0.7)
		end
		if input.isKeyPressed("b") then
			playSound("generated beep", generatedSound, 0.7)
		end
		if input.isKeyPressed("e") then
			playSound("reloaded exported beep", reloadedSound, 0.7)
		end
		if input.isKeyPressed("x") then
			runExportProbe()
			reloadedCard.imageRenderer.image = reloadedImage
		end
		if input.isKeyPressed("f") then
			runFsProbe()
		end
		if input.isKeyPressed("h") then
			runHttpProbe()
		end

		local windowWidth = math.floor(window.x + 0.5)
		local windowHeight = math.floor(window.y + 0.5)
		if windowWidth ~= lastWindowWidth or windowHeight ~= lastWindowHeight then
			lastWindowWidth = windowWidth
			lastWindowHeight = windowHeight
			markUiDirty()
		end

		local margin = 18
		panel.x = margin
		panel.y = margin
		panel.size_x = window.x - margin * 2
		panel.size_y = window.y - margin * 2

		local innerX = panel.x + 18
		local innerY = panel.y + 18
		local innerW = panel.size_x - 36

		titleEntity.x = innerX
		titleEntity.y = innerY
		titleEntity.size_x = innerW

		subtitleEntity.x = innerX
		subtitleEntity.y = innerY + 42
		subtitleEntity.size_x = innerW

		local idealCardW = math.floor((innerW - CARD_GAP * 2) / 3)
		local cardW = math.min(CARD_MAX_WIDTH, math.max(140, idealCardW))
		local cardH = cardW + 36
		local cardsTotalW = cardW * 3 + CARD_GAP * 2
		local cardsX = innerX + math.max(0, (innerW - cardsTotalW) * 0.5)
		local cardsY = innerY + 104
		local imageInset = 20
		local imageAngles = {
			math.sin(imageTime * 0.85) * 0.07,
			math.sin(imageTime * 0.68 + 1.1) * 0.11,
			math.sin(imageTime * 0.9 + 2.2) * 0.07,
		}
		local cards = { assetCard, generatedCard, reloadedCard }
		for index, card in ipairs(cards) do
			local cardX = cardsX + (index - 1) * (cardW + CARD_GAP)
			card.card.x = cardX
			card.card.y = cardsY
			card.card.size_x = cardW
			card.card.size_y = cardH

			local imageW = cardW - imageInset * 2
			local imageH = cardW - imageInset * 2
			local baseX = cardX + imageInset
			local baseY = cardsY + imageInset
			local centerX = baseX + imageW * 0.5
			local centerY = baseY + imageH * 0.5
			local angle = imageAngles[index]
			local offsetX = math.cos(angle) * imageW * 0.5 - math.sin(angle) * imageH * 0.5
			local offsetY = math.sin(angle) * imageW * 0.5 + math.cos(angle) * imageH * 0.5

			card.imageEntity.x = centerX - offsetX
			card.imageEntity.y = centerY - offsetY
			card.imageEntity.size_x = imageW
			card.imageEntity.size_y = imageH
			card.imageEntity.rotation = angle

			card.labelEntity.x = cardX + 8
			card.labelEntity.y = cardsY + cardW - 4
			card.labelEntity.size_x = cardW - 16
			card.labelEntity.size_y = 24
		end

		local controlsHeight = 48
		local previewHeight = 28
		local logHeight = 88
		local contentBottom = panel.y + panel.size_y - 12

		controlsEntity.x = innerX
		controlsEntity.y = contentBottom - controlsHeight
		controlsEntity.size_x = innerW
		controlsEntity.size_y = controlsHeight

		assetPreviewEntity.x = innerX
		assetPreviewEntity.y = controlsEntity.y - previewHeight - 8
		assetPreviewEntity.size_x = innerW
		assetPreviewEntity.size_y = previewHeight

		logEntity.x = innerX
		logEntity.y = assetPreviewEntity.y - logHeight - 10
		logEntity.size_x = innerW
		logEntity.size_y = logHeight

		summaryEntity.x = innerX
		summaryEntity.y = cardsY + cardH + 18
		summaryEntity.size_x = innerW
		summaryEntity.size_y = math.max(86, logEntity.y - summaryEntity.y - 10)

		if uiDirty then
			refreshUiText()
		end
	end,
})
