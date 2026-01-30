--!strict
--!native
--!optimize 2

local mod = {}

mod.singleton = nil
local spawner = require("../spawner")
local shrinkSpeed = 8

mod.component = {
    awake = function(entity, component)
        mod.singleton = component
        component.health = 255
        component.follow = true
    end,
    update = function(entity, component, dt)
        if component.follow then
            entity.x = math.lerp(entity.x, mouse.x, dt*15)
            entity.y = window.y - 100
        end
        if component.health <= 0 then
            ecs.deleteEntity(spawner.enemyHolder)
            component.follow = false
            spawner.cancelSpawning = true
        end

        if not component.follow then
            entity.size_x -= dt * shrinkSpeed
            entity.size_y -= dt * shrinkSpeed
            entity.x += dt * (shrinkSpeed / 2)
            entity.y += dt * (shrinkSpeed / 2)

            if entity.size_x <= 0.5 then
                ecs.deleteEntity(entity)
                print(`Game over! Score: {math.floor(spawner.t)}`)
                die()
            end
        end
        
        bg.R = math.lerp(bg.R, 255-component.health, dt*15)
        if bg.R >= 255 then
            bg.R = 0
        end
    end,
}


return mod