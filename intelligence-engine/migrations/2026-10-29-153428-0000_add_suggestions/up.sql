-- Your SQL goes here
create table suggestions (
  id integer not null primary key autoincrement, 
  session_id integer not null, 
  sent integer default 0,
  action_id text not null, 
  payload text not null,
  foreign key (session_id) references session (id)
);
