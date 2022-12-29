CREATE OR REPLACE FUNCTION notify_thread_watchers() RETURNS TRIGGER AS $new_post$
BEGIN
    PERFORM pg_notify('new_posts', row_to_json(NEW)::TEXT);
    RETURN NEW;
END;
$new_post$ language plpgsql;

CREATE TRIGGER new_post AFTER INSERT ON replies FOR EACH ROW EXECUTE FUNCTION notify_thread_watchers();
