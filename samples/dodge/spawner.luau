--!strict
--!native
--!optimize 2

local mod = {}

mod.t = 0
local spawning = false

local enemyComponent = require("./components/enemy")

mod.enemyHolder = ecs.newEntity("enemyHolder", ecs.root, 0,0)
mod.cancelSpawning = false

mod.system = {update = function(system, dt)
    mod.t += dt
    if not spawning and mod.t > 5 then
        print("incoming!")
        spawning = true
    end

    if spawning and not mod.cancelSpawning then
        if math.random(1, 5/dt/mod.t/(window.x/600)) == 1 then
            local enemy = ecs.newEntity("enemy", mod.enemyHolder, 50, 50)
            enemy.size_x = 30
            enemy.size_y = 30

            local c = ecs.addComponent(enemy, enemyComponent)
            c.t = mod.t
        end
    end
end}

return mod