package credentials

import "testing"

const testSecret = "test-secret-with-at-least-32-characters"

func TestCipherEncryptDecrypt(t *testing.T) {
	cipher, err := New(testSecret)
	if err != nil {
		t.Fatal(err)
	}

	encrypted, err := cipher.Encrypt("student-password")
	if err != nil {
		t.Fatal(err)
	}
	if encrypted == "student-password" {
		t.Fatal("encrypted value must not equal plaintext")
	}
	if !IsEncrypted(encrypted) {
		t.Fatal("encrypted value must have version prefix")
	}

	decrypted, err := cipher.Decrypt(encrypted)
	if err != nil {
		t.Fatal(err)
	}
	if decrypted != "student-password" {
		t.Fatalf("unexpected decrypted value: %q", decrypted)
	}
}

func TestCipherUsesRandomNonce(t *testing.T) {
	cipher, err := New(testSecret)
	if err != nil {
		t.Fatal(err)
	}

	first, err := cipher.Encrypt("same-password")
	if err != nil {
		t.Fatal(err)
	}
	second, err := cipher.Encrypt("same-password")
	if err != nil {
		t.Fatal(err)
	}
	if first == second {
		t.Fatal("encrypting the same value twice must produce different ciphertexts")
	}
}

func TestCipherRejectsWrongSecret(t *testing.T) {
	cipher, err := New(testSecret)
	if err != nil {
		t.Fatal(err)
	}
	encrypted, err := cipher.Encrypt("student-password")
	if err != nil {
		t.Fatal(err)
	}

	other, err := New("another-secret-with-at-least-32-chars")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := other.Decrypt(encrypted); err == nil {
		t.Fatal("decrypting with a wrong secret must fail")
	}
}

func TestCipherRejectsShortSecret(t *testing.T) {
	if _, err := New("short"); err == nil {
		t.Fatal("short secret must be rejected")
	}
}
