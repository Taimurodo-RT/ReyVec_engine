-- Жук (6.9): гуляет у своего места, у стены разворачивается. Увидел героя рядом —
-- бежит к нему и подпрыгивает. От удара его отбрасывает, как манекен; после трёх
-- ударов он разбит и через несколько секунд появляется снова.
--
-- Поменяй числа и сохрани файл: жук сразу начнёт вести себя по-новому, игру
-- перезапускать не нужно. Ошибку в файле видно на панели (файл и строка), а жук
-- пока ведёт себя по прежней версии.
--
-- me: x, y, vx, vy — где жук и как движется; on_ground — стоит; wall — стена сбоку
-- (-1 слева, 1 справа, 0 нет); hp — здоровье; alive — цел; touching — касается
-- героя; time — время, секунды. Скрипт пишет: move (от -1 до 1) и jump.
-- Свои поля можно хранить в me (me.home, me.dir): они живут, пока жив мир.
-- hero: x, y, vx, vy, striking (бьёт).

local WALK = 0.25      -- скорость прогулки (доля от наибольшей)
local CHASE = 0.6      -- скорость погони
local RANGE = 0.5      -- как далеко гуляет от своего места
local SEE = 0.9        -- как далеко видит героя
local HOP_EVERY = 1.4  -- как часто прыгает в погоне, секунды

function update(me, hero, dt)
    me.home = me.home or me.x
    me.dir = me.dir or -1
    me.hop = (me.hop or 0) - dt
    if not me.alive then
        me.move = 0
        return
    end
    if me.wall ~= 0 then
        me.dir = -me.wall
    end
    local dx = hero.x - me.x
    local near = math.abs(dx) < SEE and math.abs(hero.y - me.y) < 0.4
    if near then
        me.move = (if dx > 0 then 1 else -1) * CHASE
        if me.on_ground and me.hop <= 0 then
            me.jump = true
            me.hop = HOP_EVERY
        end
    else
        if me.x > me.home + RANGE then
            me.dir = -1
        elseif me.x < me.home - RANGE then
            me.dir = 1
        end
        me.move = me.dir * WALK
    end
end
