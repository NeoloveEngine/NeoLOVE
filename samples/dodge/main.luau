--!strict
--!native
--!optimize 2

bg = {R = 0, G = 0, B = 0}

local characterController = require("./components/character")
local spawner = require("./spawner")

local player = ecs.newEntity("player", ecs.root, 50, 50)
player.size_x = 30
player.size_y = 30

ecs.addComponent(player, characterController.component)
ecs.addSystem(spawner.system)