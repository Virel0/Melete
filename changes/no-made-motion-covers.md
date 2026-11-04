### Removed

- **Motion covers made from still covers**, and their renderer (`compose.motion.yaml`, the
  `-motion` image): they rarely looked right and took space. The videos made are deleted from
  the config folder on the first start, and their records with them. Gone from the API:
  `/api/v1/music/motion-covers`, `/api/v1/music/motion-covers/status` and albums'
  `motion_cover_generation`; the Motion covers setting and task. An album's own motion cover,
  from its folder, is still given as `motion_cover`. To tidy up: take `compose.motion.yaml` out
  of `COMPOSE_FILE`, remove the `MELETE_MOTION_*` lines from `.env`, and delete the motion jobs
  and models folders.
