from app.auth.disposable import is_disposable_email


def test_disposable_detection():
    assert is_disposable_email("x@mailinator.com")
    assert is_disposable_email("X@YOPMAIL.COM")  # case-insensitive
    assert is_disposable_email("a.b@guerrillamail.com")
    assert not is_disposable_email("x@gmail.com")
    assert not is_disposable_email("x@aallyn.net")
