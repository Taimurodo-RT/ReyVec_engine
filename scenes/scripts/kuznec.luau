-- Кузнец (Сю.5): герой подошёл — кузнец заговаривает. Какой разговор, решает сюжет
-- (папка story/): в первый раз «kuznec.privet», а когда руда собрана — «kuznec.ruda».
--
-- story.talk("диалог") начинает разговор, story.busy() — идёт ли разговор сейчас,
-- quest.stage("mech") — стадия задания «Меч для героя».

function update(me, hero, dt)
    if me.touching and not me.talked and not story.busy() then
        if quest.stage("mech") == "otnesti" then
            story.talk("kuznec.ruda")
        else
            story.talk("kuznec.privet")
        end
        me.talked = true
    end
    -- Герой отошёл: в следующий раз кузнец снова заговорит.
    if not me.touching then
        me.talked = false
    end
end
