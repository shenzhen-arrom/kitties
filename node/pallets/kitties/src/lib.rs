#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

// #[cfg(feature = "runtime-benchmarks")]
// mod benchmarking;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::{
        dispatch::DispatchResult,
        pallet_prelude::*,
        traits::{Randomness, Currency, ReservableCurrency}
    };
	use frame_system::pallet_prelude::*;
    use codec::{Encode, Decode};
    use sp_io::hashing::blake2_128;
    use sp_runtime::traits::{AtLeast32BitUnsigned, Bounded};

    #[derive(Encode, Decode)]
    pub struct Kitty(pub [u8;16]);
    type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::config]
	pub trait Config: frame_system::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        type Randomness: Randomness<Self::Hash, Self::BlockNumber>;
        // Define KittyIndex in Runtime.
        type KittyIndex: Parameter + AtLeast32BitUnsigned + Default + Copy + Bounded;
        type Currency: Currency<Self::AccountId> + ReservableCurrency<Self::AccountId>;
        // Configurable constant for the amount of staking when create a kitty,
        // to avoid the user create a big number of kitties to attract the chain.
        #[pallet::constant]
        type StakeForEachKitty: Get<BalanceOf<Self>>;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

    #[pallet::event]
	#[pallet::metadata(T::AccountId = "AccountId")]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
        KittyCreated(T::AccountId, T::KittyIndex),
        KittyTransferred(T::AccountId, T::AccountId, T::KittyIndex),
        KittyListed(T::AccountId, T::KittyIndex, Option<BalanceOf<T>>),
	}

    /// Storage for tracking all the kitties
    #[pallet::storage]
	#[pallet::getter(fn kitties_count)]
	pub type KittiesCount<T: Config> = StorageValue<_, T::KittyIndex>;

    /// Storage for every kitty.
    #[pallet::storage]
	#[pallet::getter(fn kitties)]
	pub type Kitties<T: Config> = StorageMap<_, Blake2_128Concat, T::KittyIndex, Option<Kitty>, ValueQuery>;

    /// Storage for kitties which are listed for sale.
    /// If the list price (Option<BalanceOf<T>>) is None, means the specific kitty is not for sale.
    #[pallet::storage]
	#[pallet::getter(fn kitties_list_for_sales)]
	pub type ListForSale<T: Config> = StorageMap<_, Blake2_128Concat, T::KittyIndex, Option<BalanceOf<T>>, ValueQuery>;

    /// Storage for tracking the ownership of kitties.
    #[pallet::storage]
	#[pallet::getter(fn owner)]
	pub type Owner<T: Config> = StorageMap<_, Blake2_128Concat, T::KittyIndex, Option<T::AccountId>, ValueQuery>;

	#[pallet::error]
	pub enum Error<T> {
        KittiesCountOverflow,
        NotOwner,
        SameParentIndex,
        InvalidKittyIndex,
        BuyerIsOwner,
        NotForSale,
        NotEnoughBalanceForStaking,
        NotEnoughBalanceForBuying,
	}

	#[pallet::call]
	impl<T:Config> Pallet<T> {

        /// Create a kitty with the stake configurated from:
        /// #[pallet::constant]
        ///      type StakeForEachKitty: Get<BalanceOf<Self>>)
        #[pallet::weight(1_000)]
        pub fn create(origin: OriginFor<T>) -> DispatchResult{
            let who = ensure_signed(origin)?;

            let dna = Self::random_value(&who);
            Self::new_kitty_with_stake(&who, dna)?;

            Ok(())
        }

        /// Transfer a kitty from owner to another.
        #[pallet::weight(1_000)]
        pub fn transfer(origin: OriginFor<T>, new_owner: T::AccountId, kitty_id: T::KittyIndex) -> DispatchResult {
            let who = ensure_signed(origin)?;
            // Ensure transfer only from the OWNER of kitties.
            ensure!(Some(who.clone()) == Owner::<T>::get(kitty_id), Error::<T>::NotOwner);

            let stake_amount = T::StakeForEachKitty::get();

            // Staking from new owner and unstaking from the ex-ownder
            T::Currency::reserve(&new_owner, stake_amount)
                .map_err(|_| Error::<T>::NotEnoughBalanceForStaking)?;
            T::Currency::unreserve(&who, stake_amount);

            // Update storage.
            Owner::<T>::insert(kitty_id, Some(new_owner.clone()));
            // Emit the event.
            Self::deposit_event(Event::KittyTransferred(who, new_owner, kitty_id));

            Ok(())
        }

        /// Breed a kitty from other 2 kitties (Allow the kitty parents belong to other owners).
        #[pallet::weight(1_000)]
        pub fn breed(origin: OriginFor<T>, kitty_id_1: T::KittyIndex, kitty_id_2: T::KittyIndex) -> DispatchResult {
            let who = ensure_signed(origin)?;
            // Ensure the parents are not same.
            ensure!(kitty_id_1 != kitty_id_2, Error::<T>::SameParentIndex);
            // Ensure there're the parents in the Storage.
            let kitty1 = Self::kitties(kitty_id_1).ok_or(Error::<T>::InvalidKittyIndex)?;
            let kitty2 = Self::kitties(kitty_id_2).ok_or(Error::<T>::InvalidKittyIndex)?;
            // Breed new kitty from the parents.
            let dna_1 = kitty1.0;
            let dna_2 = kitty2.0;
            let selector = Self::random_value(&who);
            let mut new_dna = [0u8; 16];
            for i in 0..dna_1.len() {
                new_dna[i] = (selector[i] & dna_1[i]) | (!selector[i] & dna_2[i]);
            }
            Self::new_kitty_with_stake(&who, new_dna)?;

            Ok(())
        }

        /// Set a price and list a kitty for sale. (Allow set None which means NOT_FOR_SALE.)
        #[pallet::weight(1_000)]
        pub fn sell(origin: OriginFor<T>, kitty_id: T::KittyIndex, price: Option<BalanceOf<T>>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            // Ensure only the kitty owner can sell it.
            ensure!(Some(who.clone()) == Owner::<T>::get(kitty_id), Error::<T>::NotOwner);
            // Set a price. If the price is None, it means the kitty is not for sale.
            ListForSale::<T>::mutate_exists(kitty_id, |p| *p = Some(price));
            // Emit event.
            Self::deposit_event(Event::KittyListed(who, kitty_id, price));

            Ok(())
        }

        /// Buy a kitty from its owner.
        #[pallet::weight(1_000)]
        pub fn buy(origin: OriginFor<T>, kitty_id: T::KittyIndex) -> DispatchResult {
            let buyer = ensure_signed(origin)?;
            let owner = Owner::<T>::get(kitty_id).unwrap();
            // Ensure the buyer is not the owner.
            ensure!(Some(buyer.clone()) != Some(owner.clone()), Error::<T>::BuyerIsOwner);
            // If the price in the ListForSale is None, the kitty is not for sale.
            let amount = ListForSale::<T>::get(kitty_id).ok_or(Error::<T>::NotForSale)?;
            // Check the buyer with enough balance to buy. Ensure the free balance can pay and stake also.
            let buyer_balance = T::Currency::free_balance(&buyer);
            let stake_amount = T::StakeForEachKitty::get();
            ensure!(buyer_balance > (amount + stake_amount), Error::<T>::NotEnoughBalanceForBuying);
            // Staking for own the kitty.
            T::Currency::reserve(&buyer, stake_amount)
                .map_err(|_| Error::<T>::NotEnoughBalanceForStaking)?;
            // Unstaking from the ex-ownder (the seller).
			T::Currency::unreserve(&owner, stake_amount);
            // Transfer the price from buyer to the seller.
			T::Currency::transfer(&buyer, &owner, amount, frame_support::traits::ExistenceRequirement::KeepAlive)?;
            // Remove from the List.
			ListForSale::<T>::remove(kitty_id);
            // Update the storage with the new owner.
            Owner::<T>::insert(kitty_id, Some(buyer.clone()));
            // Emit the event.
            Self::deposit_event(Event::KittyTransferred(owner, buyer, kitty_id));

            Ok(())
        }

    }

    // Helper functions.
    impl<T: Config> Pallet<T> {
        fn random_value(sender: &T::AccountId) -> [u8; 16] {
            let payload = (
                T::Randomness::random_seed(),
                &sender,
                <frame_system::Pallet<T>>::extrinsic_index(),
            );
            payload.using_encoded(blake2_128)
        }

        // Helper function for optimizing the codes from create() and transfer().
        fn new_kitty_with_stake(owner: &T::AccountId, dna: [u8; 16]) -> DispatchResult {

            let kitty_id = match Self::kitties_count() {
                Some(id) => {
                    ensure!(id != T::KittyIndex::max_value(), Error::<T>::KittiesCountOverflow);
                    id
                },
                None => 0u32.into()
            };

            let stake = T::StakeForEachKitty::get();

            T::Currency::reserve(&owner, stake)
                .map_err(|_| Error::<T>::NotEnoughBalanceForStaking)?;

            Kitties::<T>::insert(kitty_id, Some(Kitty(dna)));
            Owner::<T>::insert(kitty_id, Some(owner.clone()));
            KittiesCount::<T>::put(kitty_id + 1u32.into());

            Self::deposit_event(Event::KittyCreated(owner.clone(), kitty_id));

            Ok(())
        }

   }
}
