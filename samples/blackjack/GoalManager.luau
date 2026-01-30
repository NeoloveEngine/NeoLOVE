--!native
--!optimize 2

local mod = {}

mod.goal = {0,0,0}
mod.m = 1

ECS.addSystem({update = function(self, dt: number)
    if 
    math.abs(bg.R - mod.goal[1]) +
    math.abs(bg.G - mod.goal[2]) +
    math.abs(bg.B - mod.goal[3])
    < 40
    then
        mod.goal = {math.random(0,255),math.random(0,255),math.random(0,255)}
        mod.m += 0.1
        print(`new goal is now {mod.goal[1]}, {mod.goal[2]}, {mod.goal[3]}`)
        print(`multiplier is now {mod.m}`)
    end
end})

return mod