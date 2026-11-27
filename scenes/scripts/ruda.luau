-- Кусок руды (Сю.7): виден, только пока идёт стадия «руда» задания «Меч для героя».
-- Герой коснулся — кусок пропадает, а задание считает его: quest.add("mech", "ruda").
-- Три куска — стадия кончается сама, кузнец заговорит о руде (scripts/kuznec.luau).

function update(me, hero, dt)
    local need = quest.state("mech") == "active" and quest.stage("mech") == "ruda"
    if me.taken or not need then
        me.hidden = true
        return
    end
    me.hidden = false
    if me.touching then
        me.taken = true
        me.hidden = true
        story.set("hero.ruda", story.get("hero.ruda") + 1)
        quest.add("mech", "ruda")
    end
end
