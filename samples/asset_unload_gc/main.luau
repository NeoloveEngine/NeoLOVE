print("asset unload/gc test")

app.bg = Color4(20, 20, 30, 255)
app.setMaxFps(120)

local img = assets.loadImage("me.png")
local snd = assets.loadSound("sfx.wav")

local e = ecs.newEntity("sprite", ecs.root, 150, 120)
e.size_x = 200
e.size_y = 200

local r = ecs.addComponent(e, core.Image2D)
r.color = Color4(255, 255, 255, 255)
r.image = img

audio.playOnce(snd, 0.7)

local t = 0
local step = 0

ecs.addSystem({
	update = function(_self, dt)
		t += dt

		if step == 0 and t > 1 then
			print("[1s] unloading sound handle")
			assets.unloadSound(snd)
			local ok, err = pcall(function()
				audio.playOnce(snd, 0.7)
			end)
			print("play unloaded sound ok?", ok, err)
			step = 1
		end

		if step == 1 and t > 2 then
			print("[2s] reloading sound from disk")
			snd = assets.loadSound("sfx.wav")
			audio.playOnce(snd, 0.7)
			step = 2
		end

		if step == 2 and t > 3 then
			print("[3s] unloading image handle + detaching from renderer")
			assets.unloadImage(img)
			r.image = nil
			step = 3
		end

		if step == 3 and t > 4 then
			print("[4s] reloading image from disk + reattaching")
			img = assets.loadImage("me.png")
			r.image = img
			step = 4
		end

		if step == 4 and t > 5 then
			print("[5s] drop refs + assets.gc()")
			r.image = nil
			img = nil
			snd = nil

			if collectgarbage then
				collectgarbage("collect")
			end

			local images_collected, sounds_collected = assets.gc()
			print("assets.gc collected:", images_collected, sounds_collected)
			step = 5
		end

		if step == 5 and t > 6 then
			print("[6s] unload by path + reload again")
			assets.unloadImage("me.png")
			assets.unloadSound("sfx.wav")
			local img2 = assets.loadImage("me.png")
			local snd2 = assets.loadSound("sfx.wav")
			r.image = img2
			audio.playOnce(snd2, 0.7)
			step = 6
		end
	end
})

