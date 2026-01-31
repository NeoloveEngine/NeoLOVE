--!strict
--!native
--!optimize 2

bg = {R = 255, G = 255, B = 255}

local characterController = require("./components/character")
local spawner = require("./spawner")

local player = ecs.newEntity("player", ecs.root, 50, 50)
player.size_x = 30
player.size_y = 30

local healthbox = ecs.newEntity("healthbox", player, 5, 5)
healthbox.size_x = 20
healthbox.size_y = 20

healthbox.renderer = ecs.addComponent(healthbox, core.Rect2D)
player.hp = healthbox.renderer

local renderer = ecs.addComponent(player, core.Rect2D)
renderer.color = Color4(50, 50, 255)
healthbox.z = 2

local l = ecs.newEntity("label", ecs.root, 50, 50)
local textRenderer = ecs.addComponent(l, core.RudimentaryTextLabel)
textRenderer.color = Color4(0,0,0)
l.textRenderer = textRenderer
l.z = 1

ecs.addComponent(player, characterController.component)
ecs.addSystem(spawner.system)