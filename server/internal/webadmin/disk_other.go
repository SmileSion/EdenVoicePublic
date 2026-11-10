//go:build !unix

package webadmin

func diskSpace(path string) (*uint64, *uint64) { return nil, nil }
