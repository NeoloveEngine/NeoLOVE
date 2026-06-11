# NeoLOVE API Guide

## Filesystem Paths

The `fs` module accepts relative, parent-relative, and absolute paths.

- Relative paths such as `save/profile.json` use the writable game data
  directory.
- Parent-relative paths such as `../shared/profile.json` are normalized and
  may leave the game data directory.
- Absolute paths such as `/home/user/profile.json` or
  `C:\Users\User\profile.json` are used directly.

NeoLOVE does not restrict explicit paths to the project directory, game data
directory, or executable directory. Access can still fail when the operating
system denies permission, a sandbox blocks the location, or the path is
invalid.

```luau
fs.writeFile("/tmp/neolove/save.json", '{"level": 4}')
fs.appendFile("../shared/log.txt", "started\n")
fs.createDir("/tmp/neolove/maps")
```

`fs.getDataDirectory()` returns the default writable directory.
`fs.dataPath(path)` resolves a relative path against that directory; an
absolute path is returned as an absolute normalized path.

Relative reads first check the writable game data directory, then fall back to
the bundled project resources. This lets exported games load bundled defaults
while saving modified files outside the embedded project.

## Image And Sound Export

`ImageHandle:export()`/`:save()` and `SoundHandle:export()`/`:save()` follow the
same destination rules as `fs` writes. Relative paths use the game data
directory, while parent-relative and absolute paths can target any
OS-permitted location.

```luau
local image = assets.newImage(64, 64, Color4(255, 0, 0))
image:export("/tmp/neolove/generated/icon.png")

local sound = assets.newSound(44100, 1, 44100)
sound:export(fs.dataPath("generated/tone.wav"))
```

`.png` and `.wav` are added when the corresponding export path has no
extension. A different extension is rejected.

## Async Tasks

`async(callback)` creates a Luau coroutine and queues it for the engine update
loop. The callback begins on the next update. Each call to `async.yield()`
suspends it until the following update.

```luau
local task = async(function()
	for chunk = 1, 100 do
		generateMapChunk(chunk)
		async.yield()
	end
	return "finished", 100
end)
```

This is cooperative scheduling, not an operating-system thread. A callback
that performs a long operation without yielding will still block that frame.
Split CPU-heavy work into chunks and call `async.yield()` regularly. Existing
synchronous filesystem and command calls complete before the task can yield.

Task handles provide:

- `task:isDone()` and `task:getStatus()`
- `task:cancel()`
- `task:getError()`
- `task:getResult()` for all returned values
- public `done`, `cancelled`, `status`, `error`, `result`, and `results` fields

The module also provides `async.count()` and `async.cancelAll()`.
