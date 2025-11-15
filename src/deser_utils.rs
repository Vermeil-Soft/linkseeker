use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6}; 

pub struct SocketAddrCustom(pub SocketAddr);

impl std::fmt::Display for SocketAddrCustom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            SocketAddr::V4(v4) => {
                let ip = v4.ip().to_bits();
                let port = v4.port();
                write!(f, "4{:08x}{:04x}", ip, port)
            },
            SocketAddr::V6(v6) => {
                let ip = v6.ip().to_bits();
                let port = v6.port();
                write!(f, "6{:032x}{:04x}", ip, port)
            },
        }
    }
}

impl std::str::FromStr for SocketAddrCustom {
    type Err = ();

    fn from_str(input: &str) -> Result<SocketAddrCustom, ()> {
        match input.split_at_checked(1) {
            Some((first, last)) if first == "4" => {
                let Some((ip, port)) = last.split_at_checked(8) else {
                    return Err(());
                };
                let ip = u32::from_str_radix(ip, 16).map_err(|_| ())?;
                let port = u16::from_str_radix(port, 16).map_err(|_| ())?;
                Ok(SocketAddrCustom(SocketAddr::V4(SocketAddrV4::new(ip.into(), port))))
            },
            Some((first, last)) if first == "6" => {
                let Some((ip, port)) = last.split_at_checked(32) else {
                    return Err(());
                };
                let ip = u128::from_str_radix(ip, 16).map_err(|_| ())?;
                let port = u16::from_str_radix(port, 16).map_err(|_| ())?;
                Ok(SocketAddrCustom(SocketAddr::V6(SocketAddrV6::new(ip.into(), port, 0, 0))))
            },
            _ => Err(()),
        }
    }
}

pub struct VecCustom<T>(pub Vec<T>);

impl<T: std::fmt::Display> std::fmt::Display for VecCustom<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some((head, tail)) = self.0.split_first() else {
            return Ok(())
        };
        write!(f, "{}", head)?;
        for el in tail {
            write!(f, ",{}", el)?;
        }
        Ok(())
    }
}

impl<T: std::str::FromStr> std::str::FromStr for VecCustom<T> {
    type Err = ();

    fn from_str(input: &str) -> Result<VecCustom<T>, ()> {
        let mut v = Vec::new();
        if input.len() == 0 {
            return Ok(VecCustom(v));
        }
        for el in input.split(',') {
            let el = T::from_str(el).map_err(|_| ())?;
            v.push(el);
        }
        Ok(VecCustom(v))
    }
}

#[test]
#[cfg(test)]
fn deser_socket_addr() {
    use std::str::FromStr;

    let v4: SocketAddr = SocketAddr::from(([1, 2, 3, 4], 1234)).into();
    let v6: SocketAddr = SocketAddr::from(([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16], 1234)).into();

    let s4 = format!("{}", SocketAddrCustom(v4));
    let s6 = format!("{}", SocketAddrCustom(v6));

    dbg!(&s4);
    dbg!(&s6);

    let p4 = SocketAddrCustom::from_str(&s4).unwrap();
    let p6 = SocketAddrCustom::from_str(&s6).unwrap();
    assert_eq!(v4, p4.0);
    assert_eq!(v6, p6.0);
}

#[test]
#[cfg(test)]
fn deser_vec_custom() {
    use std::str::FromStr;

    let vempty: Vec<u16> = vec![];
    let v1: Vec<u16> = vec![25];

    let sempty = format!("{}", VecCustom(vempty.clone()));
    let s1 = format!("{}", VecCustom(v1.clone()));

    dbg!(&sempty);
    dbg!(&s1);

    let pempty: VecCustom<u16> = VecCustom::from_str(&sempty).unwrap();
    let p1 = VecCustom::from_str(&s1).unwrap();
    assert_eq!(vempty.len(), pempty.0.len());
    assert_eq!(v1[0], p1.0[0]);
}