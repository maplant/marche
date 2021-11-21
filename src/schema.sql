CREATE TABLE users (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL,
  password TEXT NOT NULL,
  rank_id INTEGER,
  last_reward TIMESTAMP NOT NULL
);

CREATE TABLE login_sessions (
  id SERIAL PRIMARY KEY,
  session_id VARCHAR NOT NULL,
  user_id INT NOT NULL,
  session_start TIMESTAMP NOT NULL
);

CREATE TABLE threads (
  id SERIAL PRIMARY KEY,
  author_id INT NOT NULL,
  post_date TIMESTAMP NOT NULL,
  last_post TIMESTAMP NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  reward INT
);

CREATE TABLE replies (
  id SERIAL PRIMARY KEY,
  author_id INT NOT NULL,
  thread_id INT NOT NULL,
  post_date TIMESTAMP NOT NULL,
  body TEXT NOT NULL,
  reward INT
);
