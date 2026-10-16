-- Keep navigation anchored across wizard steps and controller restarts.
ALTER TABLE telegram_bot_access ADD COLUMN menu_message_id bigint
    CHECK(menu_message_id>0 AND menu_message_id<=2147483647);
