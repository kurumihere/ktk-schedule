package config

import "testing"

func TestNormalizeHealthAddr(t *testing.T) {
	tests := []struct {
		name    string
		addr    string
		port    string
		want    string
		wantErr bool
	}{
		{
			name: "default localhost port",
			port: "8080",
			want: "127.0.0.1:8080",
		},
		{
			name: "explicit address",
			addr: "0.0.0.0:9090",
			port: "8080",
			want: "0.0.0.0:9090",
		},
		{
			name:    "invalid address",
			addr:    "0.0.0.0",
			port:    "8080",
			wantErr: true,
		},
		{
			name:    "invalid port",
			port:    "abc",
			wantErr: true,
		},
	}

	for _, tt := range tests {
		got, err := normalizeHealthAddr(tt.addr, tt.port)
		if (err != nil) != tt.wantErr {
			t.Fatalf("%s: error = %v, want error %v", tt.name, err, tt.wantErr)
		}
		if got != tt.want {
			t.Fatalf("%s: addr = %q, want %q", tt.name, got, tt.want)
		}
	}
}
