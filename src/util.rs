pub trait FastFloat {
    fn fsub(self, other: Self) -> Self;
    fn fadd(self, other: Self) -> Self;
    fn fmul(self, other: Self) -> Self;
    fn fdiv(self, other: Self) -> Self;
}

impl FastFloat for f32 {
    #[inline(always)]
    #[allow(clippy::incompatible_msrv)]
    fn fsub(self, other: Self) -> Self {
        #[cfg(rustc_1_98)]
        {
            self.algebraic_sub(other)
        }
        #[cfg(not(rustc_1_98))]
        {
            self - other
        }
    }

    #[inline(always)]
    #[allow(clippy::incompatible_msrv)]
    fn fadd(self, other: Self) -> Self {
        #[cfg(rustc_1_98)]
        {
            self.algebraic_add(other)
        }
        #[cfg(not(rustc_1_98))]
        {
            self + other
        }
    }

    #[inline(always)]
    #[allow(clippy::incompatible_msrv)]
    fn fmul(self, other: Self) -> Self {
        #[cfg(rustc_1_98)]
        {
            self.algebraic_mul(other)
        }
        #[cfg(not(rustc_1_98))]
        {
            self * other
        }
    }

    #[inline(always)]
    #[allow(clippy::incompatible_msrv)]
    fn fdiv(self, other: Self) -> Self {
        #[cfg(rustc_1_98)]
        {
            self.algebraic_div(other)
        }
        #[cfg(not(rustc_1_98))]
        {
            self / other
        }
    }
}
