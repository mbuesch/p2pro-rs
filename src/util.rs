use std::time::Duration;

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

#[allow(dead_code)]
pub fn duration_to_timeval(duration: Duration) -> libc::timeval {
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: duration.as_micros().try_into().expect("tv_usec"),
    };
    while tv.tv_usec >= 1_000_000 {
        tv.tv_sec += 1;
        tv.tv_usec -= 1_000_000;
    }
    tv
}
