-- Your SQL goes here

-- User table 
CREATE TABLE user (
  id integer not null primary key autoincrement
);

-- Session table 
CREATE TABLE session (
  id integer not null primary key autoincrement, 
  date integer not null, 
  user_id integer not null,
  foreign key(user_id) references user (id)
);
