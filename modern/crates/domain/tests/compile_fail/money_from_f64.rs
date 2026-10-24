//! R18 compile-fail case (AAP 0.3.3.1; Money constraint, R1): money cannot be built from a
//! floating-point value.
//!
//! `TRAN-CAT-BAL PIC S9(09)V99` (`app/cpy/CVTRA01Y.cpy:9`) is `Money<9>`, an exact scale-2
//! decimal. No conversion from a binary floating-point number exists, so a bare floating-point
//! literal is rejected.
//! Expected: exactly one error, at `Money::<9>::from(1.5)`.

use domain::money::Money;

fn main() {
    let _ = Money::<9>::from(1.5);
}
