### Fixed — Release train

- **A green train wakes the supervisor** — Was: after a red full set, the green run at the repaired tip sent no notice, and the max-age notice fired once per train, ever. Now: the first green full set at a new tip after a red (or with no earlier verdict) sends one release_cut_ready for that tip, and the max-age notice is keyed by tip, so a changed tip can notify again while an unchanged one never repeats.
