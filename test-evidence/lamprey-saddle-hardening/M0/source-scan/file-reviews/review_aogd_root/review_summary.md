# AOG daemon review

Three ranked daemon files were read in full, with `main.rs` reviewed as supporting reachability evidence. The daemon has a fail-open bootstrap posture: omitting both optional trust sources leaves all admin mutations unauthenticated while startup succeeds. Loopback binding is the default mitigation, but it does not protect against local processes and the supplied multi-node harness explicitly disables it.

The same daemon composition also supplies the runtime reachability evidence for AWF-001, the plaintext unauthenticated Raft transport candidate recorded by the wire review.
