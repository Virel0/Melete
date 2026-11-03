### Fixed

- AI picks, search by description and music by description could come back empty with a model
  that thinks before answering: its thinking used up the room left for the answer. Melete now
  asks OpenRouter for no thinking (kept low for a model that can't turn it off), leaves more
  room, and an empty answer is logged with the model OpenRouter used and why it stopped.
