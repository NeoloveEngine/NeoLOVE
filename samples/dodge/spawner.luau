--!strict
--!native
--!optimize 2

local mod = {}

mod.t = 0
mod.phase = 0
local spawning = false

local enemyComponent = require("./components/enemy")

mod.enemyHolder = ecs.newEntity("enemyHolder", ecs.root, 0, 0)
mod.enemyHolder.size_x = 0
mod.enemyHolder.size_y = 0
mod.cancelSpawning = false

mod.system = {update = function(system, dt)
    mod.t += dt

    local textRenderer = ecs.findFirstChild(ecs.root, "label").textRenderer

    if mod.t > 40 then
        local t_diff = mod.t - 40
        local frequency = 1 + t_diff * 0.1
        local amplitude = math.min(window.x / 2, t_diff * 10)
        
        mod.phase += dt * frequency
        mod.enemyHolder.x = amplitude * (1 - math.cos(mod.phase))
    else
        mod.enemyHolder.x = 0
    end

    if not spawning and mod.t > 5 then
        spawning = true
    elseif not spawning then
        textRenderer.text = `Incoming in {tostring(math.floor(5 - mod.t))}`
    end

    if spawning and not mod.cancelSpawning then
        local success = pcall(function()
            if math.random(1, 5/dt/mod.t/(window.x/600)) == 1 then
                local enemy = ecs.newEntity("enemy", mod.enemyHolder, 50, 50)
                enemy.size_x = 30
                enemy.size_y = 30

                local c = ecs.addComponent(enemy, enemyComponent)
                c.t = mod.t
            end
        end)
        if not success then
            print("failed when spawning!")
        end

        textRenderer.text = `Score: {math.floor(mod.t - 5)}`
    end
end}

return mod