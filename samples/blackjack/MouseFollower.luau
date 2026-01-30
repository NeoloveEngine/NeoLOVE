--!native
--!strict

return {
    awake = function(entity, component)
        component.offset = {
            x = 0, 
            y = 0
        }
    end,
    update = function(entity, component, dt: number)
        entity.x = mouse.x + component.offset.x
        entity.y = mouse.y + component.offset.y
    end,
}