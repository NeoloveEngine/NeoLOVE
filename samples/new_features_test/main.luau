local titleEntity = ecs.newEntity("title", ecs.root, 24, 60)
titleEntity.size_x = 724
titleEntity.size_y = 48
local title = ecs.addComponent(titleEntity, core.TextBox)
title.text = "NeoLOVE Feature Test"
title.color = Color4(245, 245, 245, 255)
title.scale = 46
title.size_mode = "entity"
title.align_x = "center"
title.text_scale = "fit_width"

titleEntity.rotation_pivot = "middle"

local subtitleEntity = ecs.newEntity("subtitle", ecs.root, 26, 108)
subtitleEntity.size_x = 724
subtitleEntity.size_y = 42
local subtitle = ecs.addComponent(subtitleEntity, core.TextBox)
subtitle.text = "Built-in default font + auto-fit TextScales + custom font loading + alignment"
subtitle.color = Color4(170, 220, 255, 255)
subtitle.scale = 24
subtitle.size_mode = "entity"
subtitle.align_x = "center"
subtitle.text_scale = "fit_width"

local defaultTextEntity = ecs.newEntity("defaultText", ecs.root, 24, 180)
local defaultText = ecs.addComponent(defaultTextEntity, core.TextBox)
defaultText.text = "Default font + scalar scale"
defaultText.scale = 30
defaultText.color = Color4(230, 210, 150, 255)

local customFontEntity = ecs.newEntity("customFontText", ecs.root, 24, 240)
customFontEntity.size_x = 320
customFontEntity.size_y = 36
local customFontText = ecs.addComponent(customFontEntity, core.TextBox)
customFontText.text = "Custom TTF in a fitted box"
customFontText.font = { path = "assets/fonts/ProggyClean.ttf" }
customFontText.scale = 34
customFontText.min_scale = 16
customFontText.size_mode = "entity"
customFontText.text_scale = "fit"
customFontText.align_y = "center"
customFontText.color = Color4(185, 255, 185, 255)

local metricsEntity = ecs.newEntity("metrics", ecs.root, 24, 292)
local metricsText = ecs.addComponent(metricsEntity, core.TextBox)
metricsText.text = ""
metricsText.scale = 18
metricsText.color = Color4(205, 205, 205, 255)

local controlsEntity = ecs.newEntity("controls", ecs.root, 24, 340)
controlsEntity.size_x = 724
controlsEntity.size_y = 50
local controlsText = ecs.addComponent(controlsEntity, core.TextBox)
controlsText.text = "F toggles FPS. R toggles max FPS. This line uses entity bounds plus centered word wrap."
controlsText.scale = 22
controlsText.size_mode = "entity"
controlsText.wrap = "word"
controlsText.text_scale = "fit"
controlsText.align_x = "center"
controlsText.align_y = "center"
controlsText.color = Color4(255, 255, 255, 255)

local statusEntity = ecs.newEntity("status", ecs.root, 24, 390)
statusEntity.size_x = 724
statusEntity.size_y = 28
local statusText = ecs.addComponent(statusEntity, core.TextBox)
statusText.scale = 24
statusText.size_mode = "entity"
statusText.align_x = "right"
statusText.align_y = "center"
statusText.color = Color4(255, 215, 140, 255)

local panel = ecs.newEntity("panel", ecs.root, 16, 20)
panel.size_x = 760
panel.size_y = 420
local panelShape = ecs.addComponent(panel, core.Shape2D)
panelShape.shape = "box"
panelShape.color = Color4(35, 45, 60, 220)
panelShape.z = -5

local showFps = true
local capped = true
app.setShowFps(showFps)
app.setMaxFps(60)

ecs.addSystem({
    update = function(self, dt)
        if input.isKeyPressed("f") then
            showFps = not showFps
            app.setShowFps(showFps)
        end

        if input.isKeyPressed("r") then
            capped = not capped
            if capped then
                app.setMaxFps(60)
            else
                app.setMaxFps(nil)
            end
        end

        local mode = capped and "60" or "uncapped"
        statusText.text = string.format("showFps=%s | maxFps=%s", tostring(showFps), mode)
        metricsText.text = string.format(
            "custom dx=%.1f dy=%.1f usedScale=%.1f lines=%d",
            customFontText.dx or 0,
            customFontText.dy or 0,
            customFontText.used_scale or 0,
            customFontText.line_count or 0
        )

        -- Keep the panel centered-ish on larger/smaller windows.
        panel.x = math.max(8, (window.x - panel.size_x) * 0.5)
        panel.y = math.max(8, (window.y - panel.size_y) * 0.08)

        local baseX = panel.x + 18
        titleEntity.x = baseX
        subtitleEntity.x = baseX
        defaultTextEntity.x = baseX
        customFontEntity.x = baseX
        metricsEntity.x = baseX
        controlsEntity.x = baseX
        statusEntity.x = baseX

        titleEntity.y = panel.y + 48
        subtitleEntity.y = panel.y + 92
        defaultTextEntity.y = panel.y + 158
        customFontEntity.y = panel.y + 214
        metricsEntity.y = panel.y + 264
        controlsEntity.y = panel.y + 314
        statusEntity.y = panel.y + 362
    end
})
