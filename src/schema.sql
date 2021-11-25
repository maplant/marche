CREATE TABLE users (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL,
  password TEXT NOT NULL,
  bio TEXT NOT NULL, 
  rank_id INTEGER,
  last_reward TIMESTAMP NOT NULL,
  equip_slot_prof_pic INTEGER,
  equip_slot_background INTEGER
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

CREATE TYPE rarity_enum AS ENUM (
  'common',
  'uncommon',
  'rare',
  'ultra_rare',
  'legendary'
);

CREATE TABLE items (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL,
  thumbnail TEXT NOT NULL,
  available BOOLEAN NOT NULL,
  rarity rarity_enum NOT NULL,
  item_type JSONB NOT NULL
);

CREATE TABLE drops (
  id SERIAL PRIMARY KEY,
  owner_id INT NOT NULL,
  item_id INT NOT NULL,
  pattern SMALLINT NOT NULL 
);

CREATE TABLE trade_requests (
  id SERIAL PRIMARY KEY,
  sender_id INTEGER NOT NULL,
  sender_items INTEGER[] NOT NULL,
  receiver_id INTEGER NOT NULL,
  receiver_items INTEGER[] NOT NULL
);
