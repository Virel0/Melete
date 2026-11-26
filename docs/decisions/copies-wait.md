Decision: A copy home (from the seedbox or slskd) isn't started into a library that isn't responding: the download stays completed, says it's waiting for that library and why, and is looked at again each minute without counting as a try. A copy already running stops the same way once its library is marked as not responding, or once one of its file steps takes longer than the library check's limit (15 seconds) or one write longer than a minute, which marks the library. A copy that copies no byte for 10 minutes is stalled: it's stopped, says so, and is tried again like any other failure, resuming its part files.

Why: Copies come home one at a time, so one waiting on a frozen share held up every transfer behind it, with nothing said. Waiting without counting tries keeps a long outage from failing downloads that are fine; restarting a stalled copy recovers from a seedbox connection that stopped moving without closing.

Date: 2026-11-25
