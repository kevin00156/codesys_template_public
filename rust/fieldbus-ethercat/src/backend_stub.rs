//! Non-Linux stand-in for the ethercrab backend (counterpart of Go's
//! `mapping_other.go`): same public surface, fails at `start()`, so the
//! workspace and the daemon compile on the Windows dev box.

use fieldbus_api::{
    AcyclicAccess, AxisCapability, AxisId, AxisIn, AxisOut, BusEvent, BusState, CycleClass,
    ExchangeStatus, Fieldbus, FieldbusError, OpModes, ParamAddr, SetpointKind,
};

use crate::config::EcatConfig;

pub struct EthercatBackend {
    cfg: EcatConfig,
}

impl EthercatBackend {
    pub fn new(cfg: EcatConfig) -> EthercatBackend {
        EthercatBackend { cfg }
    }
}

const UNSUPPORTED: FieldbusError =
    FieldbusError::Unsupported("EtherCAT raw sockets are linux-only; build/run on the target");

impl Fieldbus for EthercatBackend {
    type Acyclic = EthercatAcyclic;

    fn axis_count(&self) -> usize {
        self.cfg.axes.len()
    }

    fn capability(&self, _axis: AxisId) -> AxisCapability {
        AxisCapability {
            cycle: CycleClass::HardRt {
                cycle: self.cfg.cycle,
            },
            setpoint: SetpointKind::CyclicTrajectory,
            modes: OpModes {
                position: true,
                velocity: false,
                torque: false,
                homing: false,
            },
        }
    }

    fn bus_state(&self) -> BusState {
        BusState::Init
    }

    async fn start(&mut self) -> Result<(), FieldbusError> {
        Err(UNSUPPORTED)
    }

    async fn stop(&mut self) -> Result<(), FieldbusError> {
        Ok(())
    }

    async fn exchange(
        &mut self,
        _outs: &[AxisOut],
        _ins: &mut [AxisIn],
    ) -> Result<ExchangeStatus, FieldbusError> {
        Err(UNSUPPORTED)
    }

    fn poll_event(&mut self) -> Option<BusEvent> {
        None
    }

    fn acyclic(&self) -> EthercatAcyclic {
        EthercatAcyclic {}
    }
}

pub struct EthercatAcyclic {}

impl AcyclicAccess for EthercatAcyclic {
    async fn read(
        &mut self,
        _axis: AxisId,
        _addr: ParamAddr,
        _buf: &mut [u8],
    ) -> Result<usize, FieldbusError> {
        Err(UNSUPPORTED)
    }

    async fn write(
        &mut self,
        _axis: AxisId,
        _addr: ParamAddr,
        _data: &[u8],
    ) -> Result<(), FieldbusError> {
        Err(UNSUPPORTED)
    }
}
