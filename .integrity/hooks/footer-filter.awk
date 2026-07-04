# msg-filter: strip AI-attribution trailers, append the canonical footer.
# Shared by the git commit-msg hook and one-off history normalization. Idempotent.
{ lines[NR] = $0 }
END {
  n = 0
  for (i = 1; i <= NR; i++) {
    l = lines[i]
    if (l ~ /[Cc]o-?[Aa]uthored[ -]?[Bb]y.*([Cc]laude|[Aa]nthropic|noreply@anthropic|claude@anthropic|GPT|Copilot|Cursor)/) continue
    if (l ~ /^Claude-Session:/) continue
    if (l ~ /Generated with (\[)?Claude/) continue
    if (l ~ /^🤖/) continue
    if (l ~ /^Authored and reviewed by Basho Parks/) continue
    out[++n] = l
  }
  while (n > 0 && out[n] ~ /^[ \t\r]*$/) n--
  for (i = 1; i <= n; i++) print out[i]
  if (n > 0) print ""
  print "Authored and reviewed by Basho Parks, Copyright 2026"
}
