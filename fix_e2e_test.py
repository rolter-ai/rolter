import re

with open("crates/rolter-control/tests/ux_pipeline.rs", "r") as f:
    content = f.read()

import datetime

# The current UTC time is 2026-09-20T12:xx, so let's use a time within the allowed clock skew window
now = datetime.datetime.utcnow()
ts1 = now.isoformat()[:23] + "Z"
ts2 = (now + datetime.timedelta(seconds=4, milliseconds=500)).isoformat()[:23] + "Z"

search_ts1 = '"2026-09-18T10:00:00.250Z"'
search_ts2 = '"2026-09-18T10:00:04.750Z"'

replace_ts1 = f'"{ts1}"'
replace_ts2 = f'"{ts2}"'

# The timestamps are hardcoded in two places:
# 1. Inside `let batch = json!({...})`
# 2. Inside `assert_eq!(ch_rows[0].ts, "2026-09-18 10:00:00.250");` which is a string without the T and Z

search_assert1 = '"2026-09-18 10:00:00.250"'
search_assert2 = '"2026-09-18 10:00:04.750"'

replace_assert1 = f'"{ts1.replace("T", " ")[:-1]}"'
replace_assert2 = f'"{ts2.replace("T", " ")[:-1]}"'


if search_ts1 in content:
    content = content.replace(search_ts1, replace_ts1)
    content = content.replace(search_ts2, replace_ts2)
    content = content.replace(search_assert1, replace_assert1)
    content = content.replace(search_assert2, replace_assert2)
    with open("crates/rolter-control/tests/ux_pipeline.rs", "w") as f:
        f.write(content)
    print("Fixed tests/ux_pipeline.rs")
else:
    print("Not found")
