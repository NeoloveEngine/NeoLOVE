--!strict
--!native
--!optimize 2
-- native actually works by the way
-- or at least im guessing considering i have more consistent frame rates with it enabled

print("Hello, world!")

local manager = require("./GoalManager")
local follower = require("./MouseFollower")

ECS.addSystem({update = function(self, dt: number)
    bg.R = math.lerp(bg.R, manager.goal[1], dt)
    bg.G = math.lerp(bg.G, manager.goal[2], dt)
    bg.B = math.lerp(bg.B, manager.goal[3], dt)
end})

--[[
currently entities cannot have an image sprite
and cannot be recoloured (currently the colour is set to the background colour inverted)
and cannot not be rendered (unless setting the size to 0, haven't tested)
]]

local box = ECS.newEntity("box", ECS.root, 50, 50)
box.size_x = 10
box.size_y = 10

ECS.addComponent(box, {awake=function()end, update=function(me, c, dt)
    me.x += dt * 10
    me.size_x += dt * 5
end})

local player = ECS.newEntity("player", ECS.root, 250, 250)
player.size_x = 50
player.size_y = 50

local offsetFollower = ECS.newEntity("offset", ECS.root, 250, 250)
offsetFollower.size_x = 10
offsetFollower.size_y = 10

ECS.addComponent(player, follower)
local f2 = ECS.addComponent(offsetFollower, follower)

f2.offset.x = 150
f2.offset.y = 150

local x = 0