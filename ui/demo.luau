-- Свои действия кнопок экрана ui/demo.rui (шаг И.3б). Кнопка зовёт функцию по имени:
-- Button("Новая игра") { new_game() }. Правка файла сразу видна в окне.
--
-- Что функция может:
--   ui.open("Settings"), ui.back(), ui.close(), ui.quit(), ui.log("текст")
--   ui.next_lang(), ui.next_theme()
--   game.respawn() — герой в начало уровня с полным здоровьем
--   game.heal(3)   — здоровье 3 (от 1 до наибольшего)
-- game.* работает в сцене с героем (N).

function new_game()
    ui.log("Новая игра: герой в начале уровня")
    game.respawn()
    ui.close()
end
