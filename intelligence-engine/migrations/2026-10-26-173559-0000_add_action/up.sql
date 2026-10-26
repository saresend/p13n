-- Your SQL goes here
--
CREATE Table action(
  id integer not null primary key autoincrement, 
  session_id integer not null, 
  action_id text not null,
  payload text not null, -- Arbitrary JSON blob corresponding to the params
  foreign key (session_id) references session (id)
);
