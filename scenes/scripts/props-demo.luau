-- Свойства в скрипте (9.8). Скрипт один, а предметы с ним ведут себя по-разному: каждый
-- берёт размах и скорость покачивания из своих свойств в файле сцены.
--
-- me.props — свойства предмета (props в файле), me.tags — его метки (tags).
-- Английское имя читается через точку (me.props.damage), русское — в скобках:
-- me.props["размах"], me.tags["прячется"].

function update(me, hero, dt)
    me.home = me.home or me.y
    local by = me.props["размах"] or 0.05
    local speed = me.props["скорость"] or 1
    me.y = me.home + by * math.sin(me.time * speed * 2)
    -- С меткой «прячется» предмет пропадает, пока его касается герой.
    me.hidden = me.tags["прячется"] == true and me.touching
end
