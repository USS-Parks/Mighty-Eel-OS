# AOG approvals, federation, and wire review

Four assigned files were read in full. One high-severity candidate was established: the daemon ships and exposes plaintext, unauthenticated Raft RPCs while the implemented `NodeTls` mutual-TLS configurations are not integrated into either the daemon client network or server listener.

The approvals actor-authentication concern and federation anti-rollback persistence concern were not promoted because repository-wide call-site searches found no non-test runtime consumer for the vulnerable operations. Those reachability gaps are recorded in the per-file suppressions.
