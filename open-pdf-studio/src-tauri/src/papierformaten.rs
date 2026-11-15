//! Papierformaten die de app kent maar waarvoor CUPS geen eigen naam heeft:
//! de verlengde vellen (A3L, A2L, A1L, A0L) en de grote bladen A0 en A1.
//!
//! Puur en zonder printer te testen: `print_instelling` gebruikt dezelfde
//! maten voor de `lp`-opties en de Printdialoog toont ze uit dezelfde tabel.

/// Een papierformaat met een sleutel uit de Pagina-instelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Formulier {
    /// Sleutel in de Pagina-instelling en de Printdialoog ("a3l").
    pub sleutel: &'static str,
    /// Naam van het formaat ("A3L").
    pub naam: &'static str,
    /// Vel staand: korte zijde in mm.
    pub breedte_mm: u32,
    /// Vel staand: lange zijde in mm.
    pub hoogte_mm: u32,
}

impl Formulier {
    /// Maat in tienden van een millimeter, de eenheid van `dmPaperWidth` /
    /// `dmPaperLength` en van `DC_MAXEXTENT`.
    pub fn maat_tiende_mm(&self) -> (i32, i32) {
        (self.breedte_mm as i32 * 10, self.hoogte_mm as i32 * 10)
    }

    /// Past dit vel binnen een opgegeven maximum (in 0,1 mm)? Onbekende grens
    /// (`None`) telt als passend: dan beslist de printer zelf.
    pub fn past_binnen(&self, max_tiende_mm: Option<(i32, i32)>) -> bool {
        match max_tiende_mm {
            Some((max_breedte, max_lengte)) => {
                let (b, l) = self.maat_tiende_mm();
                b <= max_breedte && l <= max_lengte
            }
            None => true,
        }
    }
}

/// Breedte van een A4 in mm: zoveel langer is een verlengd vel.
pub const VERLENGING_MM: u32 = 210;

/// De extra formaten. Volgorde = volgorde van de Pagina-instelling.
pub const FORMULIEREN: [Formulier; 6] = [
    Formulier { sleutel: "a1", naam: "A1", breedte_mm: 594, hoogte_mm: 841 },
    Formulier { sleutel: "a0", naam: "A0", breedte_mm: 841, hoogte_mm: 1189 },
    Formulier { sleutel: "a3l", naam: "A3L", breedte_mm: 297, hoogte_mm: 630 },
    Formulier { sleutel: "a2l", naam: "A2L", breedte_mm: 420, hoogte_mm: 804 },
    Formulier { sleutel: "a1l", naam: "A1L", breedte_mm: 594, hoogte_mm: 1051 },
    Formulier { sleutel: "a0l", naam: "A0L", breedte_mm: 841, hoogte_mm: 1399 },
];

/// Het formulier bij een sleutel uit de Pagina-instelling.
pub fn formulier(sleutel: &str) -> Option<&'static Formulier> {
    FORMULIEREN.iter().find(|f| f.sleutel == sleutel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verlengd_vel_is_het_basisvel_plus_een_a4_breedte() {
        let basis = [("a3l", 297, 420), ("a2l", 420, 594), ("a1l", 594, 841), ("a0l", 841, 1189)];
        for (sleutel, breedte, hoogte) in basis {
            let f = formulier(sleutel).unwrap();
            assert_eq!((f.breedte_mm, f.hoogte_mm), (breedte, hoogte + VERLENGING_MM), "{sleutel}");
        }
    }

    #[test]
    fn tabel_is_staand_uniek_en_past_in_een_devmode() {
        for f in FORMULIEREN {
            assert!(f.breedte_mm < f.hoogte_mm, "{} staat niet staand", f.naam);
            // dmPaperWidth / dmPaperLength zijn i16 in 0,1 mm.
            let (b, l) = f.maat_tiende_mm();
            assert!(b <= i16::MAX as i32 && l <= i16::MAX as i32, "{} past niet in een DEVMODE", f.naam);
            assert_eq!(f.sleutel, f.naam.to_lowercase());
            assert!(f.naam.len() <= 31 && f.naam.is_ascii());
        }
    }
}
