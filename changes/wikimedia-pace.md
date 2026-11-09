### Fixed

- Artists' portraits keep to about one request a second to Wikidata and Commons, all of them
  together, and hold back for as long as Wikimedia asks when it answers 429 or 503, instead of
  asking in bursts. A portrait Wikimedia asked a pause for is asked for again later rather than
  believed missing for a day.
