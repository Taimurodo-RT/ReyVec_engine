-- Монета (6.9): покачивается на месте. Герой коснулся — исчезает со звоном и
-- блёстками, через несколько секунд появляется снова.
--
-- me: x, y — где монета; touching — её касается герой; time — время, секунды.
-- Скрипт пишет: y (новое место), hidden (не видна), collected (подобрана: движок
-- один раз даёт звон и блёстки).

local BACK = 4.0   -- через сколько секунд монета снова на месте
local BOB = 0.02   -- размах покачивания

function update(me, hero, dt)
    me.home = me.home or me.y
    me.gone = me.gone or 0
    if me.gone > 0 then
        me.gone -= dt
        if me.gone <= 0 then
            me.hidden = false
            me.collected = false
        end
        return
    end
    me.y = me.home + BOB * math.sin(me.time * 3 + me.x * 5)
    if me.touching then
        me.collected = true
        me.hidden = true
        me.gone = BACK
    end
end
