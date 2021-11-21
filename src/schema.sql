CREATE TABLE users (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL,
  password TEXT NOT NULL,
  rank_id INTEGER
);

CREATE TABLE login_sessions (
  id SERIAL PRIMARY KEY,
  user_id INT NOT NULL,
  session_start TIMESTAMP NOT NULL
);

CREATE TABLE threads (
  id SERIAL PRIMARY KEY,
  author_id INT NOT NULL,
  post_date TIMESTAMP NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL
);
