CREATE TYPE role_enum as ENUM (
  'admin',
  'moderator',
  'user'
);

CREATE TABLE users (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  password TEXT NOT NULL,
  bio TEXT NOT NULL, 
  role role_enum NOT NULL,
  experience BIGINT NOT NULL, 
  last_reward TIMESTAMP NOT NULL,
  equip_slot_prof_pic INTEGER,
  equip_slot_background INTEGER,
  equip_slot_badges INTEGER[] NOT NULL,
  banned_until TIMESTAMP,
  notes TEXT NOT NULL,
);

create TABLE reading_history (
  id SERIAL PRIMARY KEY,
  reader_id INT NOT NULL,
  thread_id INT NOT NULL,
  last_read INT NOT NULL,
  UNIQUE (reader_id, thread_id)
);

CREATE TABLE login_sessions (
  id SERIAL PRIMARY KEY,
  session_id VARCHAR NOT NULL,
  user_id INT NOT NULL,
  session_start TIMESTAMP NOT NULL,
  ip_addr CIDR NOT NULL
);

CREATE TABLE threads (
  id SERIAL PRIMARY KEY,
  last_post INTEGER NOT NULL,
  title TEXT NOT NULL,
  tags INTEGER[] NOT NULL,
  num_replies INTEGER NOT NULL,
  pinned BOOLEAN NOT NULL,
  locked BOOLEAN NOT NULL
);

CREATE TABLE replies (
  id SERIAL PRIMARY KEY,
  author_id INT NOT NULL,
  thread_id INT NOT NULL,
  post_date TIMESTAMP NOT NULL,
  body TEXT NOT NULL,
  reward INT,
  reactions INTEGER[] NOT NULL,
  image TEXT,
  thumbnail TEXT,
  filename TEXT
);

CREATE TABLE tags (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  num_tagged INTEGER NOT NULL DEFAULT 1
)

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
  available BOOLEAN NOT NULL,
  rarity rarity_enum NOT NULL,
  item_type JSONB NOT NULL,
  attribute_map JSONB NOT NULL
);

CREATE TABLE drops (
  id SERIAL PRIMARY KEY,
  owner_id INT NOT NULL,
  item_id INT NOT NULL,
  pattern SMALLINT NOT NULL,
  consumed BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE trade_requests (
  id SERIAL PRIMARY KEY,
  sender_id INTEGER NOT NULL,
  sender_items INTEGER[] NOT NULL,
  receiver_id INTEGER NOT NULL,
  receiver_items INTEGER[] NOT NULL,
  note TEXT
);
