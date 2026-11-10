//go:build unix

package webadmin

import "golang.org/x/sys/unix"

func diskSpace(path string) (*uint64, *uint64) {
	var s unix.Statfs_t
	if unix.Statfs(path, &s) != nil {
		return nil, nil
	}
	free, total := uint64(s.Bavail)*uint64(s.Bsize), uint64(s.Blocks)*uint64(s.Bsize)
	return &free, &total
}
