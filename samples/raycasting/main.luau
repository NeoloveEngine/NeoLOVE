app.bg = Color4(10, 14, 20, 255)
app.setMaxFps(120)

math.randomseed(os.time())

local TILE_SIZE = 56
local FOV = math.rad(72)
local RAY_COUNT = 128
local MINIMAP_RAY_COUNT = 18
local PLAYER_RADIUS = 11
local MOVE_SPEED = 175
local TURN_SPEED = 2.0
local MOUSE_SENSITIVITY = 2.4
local MAZE_CELLS_X = 14
local MAZE_CELLS_Y = 11
local HUGE = 1000000000

local function wrapAngle(angle)
	local twoPi = math.pi * 2
	while angle < 0 do
		angle += twoPi
	end
	while angle >= twoPi do
		angle -= twoPi
	end
	return angle
end

local function createRect(name, z, color)
	local e = ecs.newEntity(name, ecs.root, 0, 0)
	e.z = z
	e.raycastable = false
	local renderer = ecs.addComponent(e, core.Rect2D)
	renderer.color = color
	return e, renderer
end

local function gridToWorldCenter(gridX, gridY)
	return (gridX - 0.5) * TILE_SIZE, (gridY - 0.5) * TILE_SIZE
end

local function generateMaze(cellW, cellH)
	local mazeW = cellW * 2 + 1
	local mazeH = cellH * 2 + 1

	local grid = {}
	for y = 1, mazeH do
		grid[y] = {}
		for x = 1, mazeW do
			grid[y][x] = "#"
		end
	end

	local visited = {}
	for y = 1, cellH do
		visited[y] = {}
	end

	local function carveCell(cx, cy)
		grid[cy * 2][cx * 2] = "."
	end

	local stack = { { x = 1, y = 1 } }
	visited[1][1] = true
	carveCell(1, 1)

	local dirs = {
		{ x = 1, y = 0 },
		{ x = -1, y = 0 },
		{ x = 0, y = 1 },
		{ x = 0, y = -1 },
	}

	while #stack > 0 do
		local current = stack[#stack]
		local neighbors = {}

		for _, dir in ipairs(dirs) do
			local nx = current.x + dir.x
			local ny = current.y + dir.y
			if nx >= 1 and nx <= cellW and ny >= 1 and ny <= cellH and not visited[ny][nx] then
				neighbors[#neighbors + 1] = {
					nx = nx,
					ny = ny,
					dx = dir.x,
					dy = dir.y,
				}
			end
		end

		if #neighbors == 0 then
			table.remove(stack)
		else
			local pick = neighbors[math.random(1, #neighbors)]
			grid[current.y * 2 + pick.dy][current.x * 2 + pick.dx] = "."
			carveCell(pick.nx, pick.ny)
			visited[pick.ny][pick.nx] = true
			stack[#stack + 1] = { x = pick.nx, y = pick.ny }
		end
	end

	local extraCuts = math.floor(cellW * cellH * 0.12)
	for _ = 1, extraCuts do
		local cx = math.random(2, cellW - 1)
		local cy = math.random(2, cellH - 1)
		local wallChoices = {
			{ x = cx * 2 - 1, y = cy * 2 },
			{ x = cx * 2 + 1, y = cy * 2 },
			{ x = cx * 2, y = cy * 2 - 1 },
			{ x = cx * 2, y = cy * 2 + 1 },
		}
		local pick = wallChoices[math.random(1, #wallChoices)]
		grid[pick.y][pick.x] = "."
	end

	local startGridX, startGridY = 2, 2
	local exitGridX, exitGridY = mazeW - 1, mazeH - 1
	grid[startGridY][startGridX] = "."
	grid[exitGridY][exitGridX] = "."

	return grid, mazeW, mazeH, startGridX, startGridY, exitGridX, exitGridY
end

local maze, MAP_WIDTH, MAP_HEIGHT, START_X, START_Y, EXIT_X, EXIT_Y = generateMaze(MAZE_CELLS_X, MAZE_CELLS_Y)
local MAX_RAY_DISTANCE = TILE_SIZE * math.max(MAP_WIDTH, MAP_HEIGHT)

local function castMazeRay(originX, originY, rayAngle, maxDistance)
	local dirX = math.cos(rayAngle)
	local dirY = math.sin(rayAngle)
	local mapX = math.floor(originX / TILE_SIZE)
	local mapY = math.floor(originY / TILE_SIZE)

	local deltaDistX = math.abs(dirX) < 0.00001 and HUGE or (TILE_SIZE / math.abs(dirX))
	local deltaDistY = math.abs(dirY) < 0.00001 and HUGE or (TILE_SIZE / math.abs(dirY))

	local stepX = 0
	local stepY = 0
	local sideDistX = HUGE
	local sideDistY = HUGE

	if dirX < -0.00001 then
		stepX = -1
		sideDistX = (originX - mapX * TILE_SIZE) / -dirX
	elseif dirX > 0.00001 then
		stepX = 1
		sideDistX = ((mapX + 1) * TILE_SIZE - originX) / dirX
	end

	if dirY < -0.00001 then
		stepY = -1
		sideDistY = (originY - mapY * TILE_SIZE) / -dirY
	elseif dirY > 0.00001 then
		stepY = 1
		sideDistY = ((mapY + 1) * TILE_SIZE - originY) / dirY
	end

	local maxSteps = MAP_WIDTH + MAP_HEIGHT + 24
	for _ = 1, maxSteps do
		local distance
		local normalX = 0
		local normalY = 0

		if sideDistX < sideDistY then
			mapX += stepX
			distance = sideDistX
			sideDistX += deltaDistX
			normalX = -stepX
		else
			mapY += stepY
			distance = sideDistY
			sideDistY += deltaDistY
			normalY = -stepY
		end

		if distance > maxDistance then
			return nil, nil, nil
		end

		local gridX = mapX + 1
		local gridY = mapY + 1
		if gridX < 1 or gridX > MAP_WIDTH or gridY < 1 or gridY > MAP_HEIGHT then
			return nil, nil, nil
		end

		if maze[gridY][gridX] == "#" then
			return distance, normalX, normalY
		end
	end

	return nil, nil, nil
end

local sky, skyRenderer = createRect("sky", -40, Color4(24, 34, 54, 255))
local floor, floorRenderer = createRect("floor", -39, Color4(18, 23, 19, 255))
local minimapBg, minimapBgRenderer = createRect("minimap_bg", -20, Color4(10, 14, 16, 232))
local minimapBorder, minimapBorderRenderer = createRect("minimap_border", -19, Color4(70, 99, 76, 255))

local columns = {}
for i = 1, RAY_COUNT do
	local e, renderer = createRect("ray_column_" .. tostring(i), 4, Color4(130, 166, 124, 255))
	columns[i] = { entity = e, renderer = renderer }
end

local wallSprites = {}
local wallCount = 0
for y = 1, MAP_HEIGHT do
	for x = 1, MAP_WIDTH do
		if maze[y][x] == "#" then
			wallCount += 1
			local tint = math.random(0, 26)
			local mini, miniRenderer = createRect("hedge_mini_" .. tostring(wallCount), 12, Color4(46 + tint, 88 + tint, 52 + tint, 255))
			wallSprites[#wallSprites + 1] = {
				entity = mini,
				renderer = miniRenderer,
				gridX = x,
				gridY = y,
			}
		end
	end
end

local minimapRays = {}
for i = 1, MINIMAP_RAY_COUNT do
	local e, renderer = createRect("minimap_ray_" .. tostring(i), 14, Color4(213, 244, 142, 90))
	minimapRays[i] = { entity = e, renderer = renderer }
end

local playerMarker, playerMarkerRenderer = createRect("player_marker", 15, Color4(165, 232, 255, 255))
local playerFacing, playerFacingRenderer = createRect("player_facing", 16, Color4(235, 246, 255, 255))
local exitMarker, exitMarkerRenderer = createRect("exit_marker", 13, Color4(190, 255, 164, 255))

local crossX, crossXRenderer = createRect("cross_x", 30, Color4(230, 245, 230, 220))
local crossY, crossYRenderer = createRect("cross_y", 30, Color4(230, 245, 230, 220))

local title = ecs.newEntity("title", ecs.root, 16, 30)
title.raycastable = false
local titleLabel = ecs.addComponent(title, core.TextBox)
titleLabel.scale = 32
titleLabel.color = Color4(205, 238, 198, 255)
titleLabel.text = "Hedge Maze"

local hint = ecs.newEntity("hint", ecs.root, 16, 58)
hint.raycastable = false
local hintLabel = ecs.addComponent(hint, core.TextBox)
hintLabel.scale = 18
hintLabel.color = Color4(160, 195, 150, 255)
hintLabel.text = "W/S move, A/D strafe, mouse look. Click/Enter lock mouse, Esc release."

local stats = ecs.newEntity("stats", ecs.root, 16, 82)
stats.raycastable = false
local statsLabel = ecs.addComponent(stats, core.TextBox)
statsLabel.scale = 17
statsLabel.color = Color4(150, 185, 141, 255)

local minimapLabel = ecs.newEntity("minimap_label", ecs.root, 0, 0)
minimapLabel.raycastable = false
local minimapLabelRenderer = ecs.addComponent(minimapLabel, core.TextBox)
minimapLabelRenderer.scale = 20
minimapLabelRenderer.color = Color4(184, 220, 179, 255)
minimapLabelRenderer.text = "Maze Map"

local lockHint = ecs.newEntity("lock_hint", ecs.root, 0, 0)
lockHint.raycastable = false
local lockHintLabel = ecs.addComponent(lockHint, core.TextBox)
lockHintLabel.scale = 20
lockHintLabel.color = Color4(216, 247, 192, 255)

local playerX, playerY = gridToWorldCenter(START_X, START_Y)
local playerAngle = 0.0
local mouseLocked = false
local exitWorldX, exitWorldY = gridToWorldCenter(EXIT_X, EXIT_Y)

local layout = {
	viewWidth = 0,
	sidebarWidth = 0,
	mapScale = 1,
	mapOffsetX = 0,
	mapOffsetY = 0,
}

local lastWindowX = -1
local lastWindowY = -1

local function isWallAt(worldX, worldY)
	local gridX = math.floor(worldX / TILE_SIZE) + 1
	local gridY = math.floor(worldY / TILE_SIZE) + 1

	if gridX < 1 or gridX > MAP_WIDTH or gridY < 1 or gridY > MAP_HEIGHT then
		return true
	end

	return maze[gridY][gridX] == "#"
end

local function canMoveTo(worldX, worldY)
	local r = PLAYER_RADIUS
	return not (
		isWallAt(worldX - r, worldY - r)
		or isWallAt(worldX + r, worldY - r)
		or isWallAt(worldX - r, worldY + r)
		or isWallAt(worldX + r, worldY + r)
	)
end

local function setMouseLocked(locked)
	mouseLocked = locked
	input.setMouseLocked(locked)
end

local function refreshLayout()
	local sidebarWidth = math.max(220, window.x * 0.3)
	sidebarWidth = math.min(sidebarWidth, window.x * 0.48)
	local viewWidth = window.x - sidebarWidth - 10
	if viewWidth < 180 then
		viewWidth = window.x * 0.58
		sidebarWidth = window.x - viewWidth - 10
	end

	layout.viewWidth = viewWidth
	layout.sidebarWidth = sidebarWidth

	local sidebarX = viewWidth + 6
	local minimapX = sidebarX + 8
	local minimapY = 52
	local minimapW = math.max(104, sidebarWidth - 16)
	local minimapH = math.max(96, window.y - minimapY - 8)

	sky.x = 0
	sky.y = 0
	sky.size_x = viewWidth
	sky.size_y = window.y * 0.5
	floor.x = 0
	floor.y = window.y * 0.5
	floor.size_x = viewWidth
	floor.size_y = window.y * 0.5

	minimapBorder.x = sidebarX
	minimapBorder.y = 44
	minimapBorder.size_x = sidebarWidth
	minimapBorder.size_y = window.y - 44
	minimapBg.x = minimapX
	minimapBg.y = minimapY
	minimapBg.size_x = minimapW
	minimapBg.size_y = minimapH

	minimapLabel.x = sidebarX + 12
	minimapLabel.y = 34

	local mapWorldW = MAP_WIDTH * TILE_SIZE
	local mapWorldH = MAP_HEIGHT * TILE_SIZE
	local mapScale = math.min((minimapW - 12) / mapWorldW, (minimapH - 12) / mapWorldH)
	mapScale = math.max(mapScale, 0.06)
	layout.mapScale = mapScale

	local mapDrawW = mapWorldW * mapScale
	local mapDrawH = mapWorldH * mapScale
	layout.mapOffsetX = minimapX + (minimapW - mapDrawW) * 0.5
	layout.mapOffsetY = minimapY + (minimapH - mapDrawH) * 0.5

	for _, sprite in ipairs(wallSprites) do
		sprite.entity.x = layout.mapOffsetX + (sprite.gridX - 1) * TILE_SIZE * mapScale
		sprite.entity.y = layout.mapOffsetY + (sprite.gridY - 1) * TILE_SIZE * mapScale
		sprite.entity.size_x = TILE_SIZE * mapScale
		sprite.entity.size_y = TILE_SIZE * mapScale
	end

	exitMarker.x = layout.mapOffsetX + exitWorldX * mapScale - math.max(4, mapScale * 6)
	exitMarker.y = layout.mapOffsetY + exitWorldY * mapScale - math.max(4, mapScale * 6)
	exitMarker.size_x = math.max(8, mapScale * 12)
	exitMarker.size_y = math.max(8, mapScale * 12)

	local columnWidth = viewWidth / RAY_COUNT
	for i = 1, RAY_COUNT do
		columns[i].entity.x = (i - 1) * columnWidth
		columns[i].entity.size_x = columnWidth + 1
	end

	local minimapRayThickness = math.max(1, mapScale * 1.7)
	for i = 1, MINIMAP_RAY_COUNT do
		minimapRays[i].entity.size_y = minimapRayThickness
	end
end

setMouseLocked(false)
refreshLayout()

ecs.addSystem({
	update = function(_self, dt)
		if input.isKeyPressed("escape") then
			setMouseLocked(false)
		end
		if not mouseLocked and (input.isMousePressed("left") or input.isKeyPressed("enter")) then
			setMouseLocked(true)
		end

		if window.x ~= lastWindowX or window.y ~= lastWindowY then
			lastWindowX = window.x
			lastWindowY = window.y
			refreshLayout()
		end

		local turnInput = 0
		if input.isKeyDown("left") or input.isKeyDown("q") then
			turnInput -= 1
		end
		if input.isKeyDown("right") or input.isKeyDown("e") then
			turnInput += 1
		end
		playerAngle = wrapAngle(playerAngle + turnInput * TURN_SPEED * dt)

		if mouseLocked then
			local mouseDX = input.getMouseDelta()
			playerAngle = wrapAngle(playerAngle - mouseDX * MOUSE_SENSITIVITY)
		end

		local moveInput = 0
		if input.isKeyDown("w") or input.isKeyDown("up") then
			moveInput += 1
		end
		if input.isKeyDown("s") or input.isKeyDown("down") then
			moveInput -= 1
		end

		local strafeInput = 0
		if input.isKeyDown("d") then
			strafeInput += 1
		end
		if input.isKeyDown("a") then
			strafeInput -= 1
		end

		local forwardX = math.cos(playerAngle)
		local forwardY = math.sin(playerAngle)
		local strafeX = math.cos(playerAngle + math.pi * 0.5)
		local strafeY = math.sin(playerAngle + math.pi * 0.5)

		local velocityX = (forwardX * moveInput + strafeX * strafeInput) * MOVE_SPEED * dt
		local velocityY = (forwardY * moveInput + strafeY * strafeInput) * MOVE_SPEED * dt

		local nextX = playerX + velocityX
		if canMoveTo(nextX, playerY) then
			playerX = nextX
		end

		local nextY = playerY + velocityY
		if canMoveTo(playerX, nextY) then
			playerY = nextY
		end

		local playerMiniX = layout.mapOffsetX + playerX * layout.mapScale
		local playerMiniY = layout.mapOffsetY + playerY * layout.mapScale
		local markerSize = math.max(4, layout.mapScale * PLAYER_RADIUS * 0.6)

		playerMarker.x = playerMiniX - markerSize * 0.5
		playerMarker.y = playerMiniY - markerSize * 0.5
		playerMarker.size_x = markerSize
		playerMarker.size_y = markerSize
		playerFacing.x = playerMiniX
		playerFacing.y = playerMiniY
		playerFacing.size_x = math.max(10, layout.mapScale * 23)
		playerFacing.size_y = math.max(2, layout.mapScale * 2)
		playerFacing.rotation = playerAngle

		local projectionDistance = (layout.viewWidth * 0.5) / math.tan(FOV * 0.5)
		local hitsThisFrame = 0

		for i = 1, RAY_COUNT do
			local fraction = (i - 0.5) / RAY_COUNT
			local rayAngle = playerAngle - FOV * 0.5 + fraction * FOV
			local hitDistance, hitNormalX = castMazeRay(playerX, playerY, rayAngle, MAX_RAY_DISTANCE)
			local column = columns[i]

			if hitDistance then
				hitsThisFrame += 1
				local correctedDistance = math.max(0.001, hitDistance * math.cos(rayAngle - playerAngle))
				local wallHeight = math.min(window.y, (TILE_SIZE * projectionDistance) / correctedDistance)
				column.entity.y = (window.y - wallHeight) * 0.5
				column.entity.size_y = wallHeight

				local brightness = math.clamp(225 - correctedDistance * 0.42, 35, 220)
				local sideShade = math.abs(hitNormalX) > 0.5 and 0.78 or 1.0
				local green = (brightness * 0.95 + 20) * sideShade
				local red = (brightness * 0.42 + 14) * sideShade
				local blue = (brightness * 0.38 + 12) * sideShade
				column.renderer.color = Color4(red, green, blue, 255)
			else
				column.entity.y = window.y * 0.5
				column.entity.size_y = 0
				column.renderer.color = Color4(0, 0, 0, 0)
			end
		end

		for i = 1, MINIMAP_RAY_COUNT do
			local t = (i - 1) / math.max(1, MINIMAP_RAY_COUNT - 1)
			local rayAngle = playerAngle - FOV * 0.5 + t * FOV
			local hitDistance = castMazeRay(playerX, playerY, rayAngle, MAX_RAY_DISTANCE)
			local line = minimapRays[i]
			line.entity.x = playerMiniX
			line.entity.y = playerMiniY
			line.entity.rotation = rayAngle

			local rayDistance = MAX_RAY_DISTANCE
			if hitDistance then
				rayDistance = hitDistance
				line.renderer.color = Color4(213, 244, 142, 96)
			else
				line.renderer.color = Color4(150, 176, 150, 46)
			end
			line.entity.size_x = rayDistance * layout.mapScale
		end

		crossX.x = layout.viewWidth * 0.5 - 8
		crossX.y = window.y * 0.5 - 1
		crossX.size_x = 16
		crossX.size_y = 2
		crossY.x = layout.viewWidth * 0.5 - 1
		crossY.y = window.y * 0.5 - 8
		crossY.size_x = 2
		crossY.size_y = 16

		local lockState = mouseLocked and "locked" or "free"
		local exitDistance = math.sqrt((exitWorldX - playerX) * (exitWorldX - playerX) + (exitWorldY - playerY) * (exitWorldY - playerY))
		local fps = dt > 0 and math.floor(1 / dt + 0.5) or 0
		statsLabel.text = string.format(
			"fps %d  maze %dx%d  walls %d  rays %d  lock %s  exit %.0f",
			fps,
			MAP_WIDTH,
			MAP_HEIGHT,
			wallCount,
			RAY_COUNT,
			lockState,
			exitDistance
		)

		lockHint.x = layout.viewWidth * 0.5 - 165
		lockHint.y = window.y - 20
		lockHintLabel.text = mouseLocked and "Esc to release mouse" or "Click viewport or press Enter to lock mouse"
		crossXRenderer.color = mouseLocked and Color4(230, 245, 230, 220) or Color4(230, 245, 230, 0)
		crossYRenderer.color = mouseLocked and Color4(230, 245, 230, 220) or Color4(230, 245, 230, 0)
	end,
})
