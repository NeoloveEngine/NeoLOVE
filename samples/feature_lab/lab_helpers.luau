local helpers = {}

function helpers.clampByte(value)
	return math.floor(math.clamp(value, 0, 255))
end

function helpers.short(value, limit)
	local text = tostring(value or ""):gsub("[%c]+", " ")
	if #text <= limit then
		return text
	end
	return text:sub(1, math.max(0, limit - 3)) .. "..."
end

function helpers.countKeys(value)
	local count = 0
	for _ in pairs(value) do
		count += 1
	end
	return count
end

return helpers
