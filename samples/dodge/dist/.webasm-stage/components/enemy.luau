--!strict
--!native
--!optimize 2

return {
    awake = function(entity, component) 
        entity.x = math.random(0, window.x)
        entity.y = -30
        component.character = require("./character")
        local renderer = ecs.addComponent(entity, core.Rect2D)
        renderer.color = Color4(255, 50, 50)
    end,
    update = function(entity, component, dt)
        entity.y += component.t * dt * 20

        if entity.y - 80 > window.y then
            ecs.deleteEntity(entity)
        end

        if transform.doTheyOverlap({entity, component.character.singleton.entity}) then
            component.character.singleton.health -= component.t
            ecs.deleteEntity(entity)
        end
    end,
}